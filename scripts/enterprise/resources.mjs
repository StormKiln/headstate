import {summarizeMixedResources} from './mixed-resources.mjs';
import assert from 'node:assert/strict';

export const resourceCoverage=Object.freeze({
 readBytes:'application returned bytes: metadata,usage,index,links; excludes directory/stat/fingerprint, SQLite, network and other reads',
 transactions:['queue:deferred','corpus-metadata:immediate','corpus-usage:immediate','corpus-index:immediate','corpus-discovery:immediate','corpus-links:immediate','corpus-admission:immediate','corpus-pause:immediate'],
 mutationSemantics:'committed SQLite row callbacks only; FTS shadow rows are implementation rows; WITHOUT ROWID and unscoped writes unknown',
 waitSemantics:'BEGIN admission; deferred queue hold includes later writer upgrade contention; immediate corpus wait precedes hold',
 walSemantics:'sampled retained file length, never cumulative writes',
 memorySemantics:'native RSS and Chromium renderer JS heap include telemetry; application-only/browser-process/WebKit heap unknown',
 holes:['unscoped writes','SQLite WITHOUT ROWID callbacks','SQLite REPLACE implicit deletes','SQLite truncate DELETE optimization','directory enumeration','stat/fingerprint reads','SQLite reads','physical disk IO'],
});
export const phaseContracts=Object.freeze({startup:['startup'],mixed:['idle','unchanged_refresh','active_ingestion','churn_restart'],mixed_initial:['idle','unchanged_refresh','active_ingestion','churn_restart'],mixed_resumed:['churn_restart']});
const median=values=>{const sorted=[...values].sort((a,b)=>a-b);return sorted.length%2?sorted[Math.floor(sorted.length/2)]:(sorted[sorted.length/2-1]+sorted[sorted.length/2])/2;};
const number=(v,label)=>{assert.ok(Number.isSafeInteger(v)&&v>=0,`${label}: nonnegative safe integer required`);return v;};
function measured(v,label,{absent=false}={}){
 assert.ok(v&&typeof v==='object',`${label}: capability required`);
 if(absent&&v.status==='absent'){assert.equal(v.bytes,undefined);return null;}
 assert.equal(v.status,'measured',`${label}: required measurement unavailable`);return number(v.bytes,label);
}
// Native boundary timestamps fall inside these parent request/response brackets.
// No guessed offset between the Rust and Node monotonic clocks is treated as exact.
export function bindBrowserClock(clock,native){
 return {clock:clock.clock,phases:clock.phases.map(mapping=>({...mapping,...Object.fromEntries(['start','end'].map(boundary=>{
  const matches=native.filter(row=>row.resource?.kind==='sample'&&row.resource.phase===mapping.phase&&row.resource.boundary===boundary);
  assert.equal(matches.length,1,'unique native phase boundary required for clock mapping');
  return [boundary,{...mapping[boundary],nativeSeq:matches[0].seq,nativeNs:matches[0].ns}];
 }))}))};
}
function validateBrowserEvidence(evidence,required){
 const finite=(value,label)=>{assert.ok(Number.isFinite(value)&&value>=0,`${label}: finite nonnegative time required`);};
 assert.equal(evidence.browserClock?.clock,'node-performance-ms','browser clock domain required');
 const mappings=evidence.browserClock.phases;
 assert.ok(Array.isArray(mappings));assert.deepEqual(mappings.map(m=>m.phase).sort(),[...required].sort(),'exact browser phase mappings required');
 assert.ok(Array.isArray(evidence.browser)&&evidence.browser.length>0,'raw browser samples required');
 let previous=-1;
 for(const [index,row] of evidence.browser.entries()){
  assert.equal(row.seq,index+1,'browser sample sequence must be contiguous and unique');finite(row.at,'browser sample timestamp');
  assert.ok(row.at>=previous,'browser sample clock must be monotonic');previous=row.at;
  assert.ok(['desktop','paired'].includes(row.role),'browser sample role');assert.ok(required.includes(row.phase),'browser sample phase');
  assert.ok(['start','interval','end'].includes(row.boundary),'browser sample boundary');
 }
 for(const mapping of mappings){
  const native={};
  for(const boundary of ['start','end']){
   const clock=mapping[boundary];assert.ok(clock,'browser boundary mapping required');finite(clock.beforeMs,'control request');finite(clock.afterMs,'control response');
   assert.ok(clock.afterMs>=clock.beforeMs&&clock.afterMs-clock.beforeMs<=2500,'bounded control clock bracket required');
   const rows=evidence.native.filter(row=>row.resource?.kind==='sample'&&row.resource.phase===mapping.phase&&row.resource.boundary===boundary);
   assert.equal(rows.length,1);native[boundary]=rows[0];assert.equal(clock.nativeSeq,rows[0].seq,'native boundary sequence mapping');assert.equal(clock.nativeNs,rows[0].ns,'native boundary clock mapping');
  }
  assert.ok(mapping.end.beforeMs>mapping.start.afterMs,'ordered browser phase brackets');
  const duration=(native.end.ns-native.start.ns)/1e6;
  assert.ok(duration>=mapping.end.beforeMs-mapping.start.afterMs-2&&duration<=mapping.end.afterMs-mapping.start.beforeMs+2,'native/browser clock duration must reconcile within request brackets');
  for(const role of ['desktop','paired']){
   const rows=evidence.browser.filter(row=>row.phase===mapping.phase&&row.role===role);
   assert.ok(rows.length>=2,'browser boundary samples required');assert.equal(rows[0].boundary,'start','browser phase start sample required');assert.equal(rows.at(-1).boundary,'end','browser phase end sample required');
   assert.ok(rows.slice(1,-1).every(row=>row.boundary==='interval'),'exactly one start/end per role and phase');
   assert.ok(rows[0].at>=mapping.start.afterMs&&rows[0].at-mapping.start.afterMs<=2500,'browser start coverage');
   assert.ok(rows.at(-1).at<=mapping.end.beforeMs&&mapping.end.beforeMs-rows.at(-1).at<=2500,'browser end coverage');
   for(let i=1;i<rows.length;i++)assert.ok(rows[i].at-rows[i-1].at<=2500,'browser sampler gap exceeds 2.5 intervals');
  }
 }
}
export function summarizeResources(evidence,engine){
 assert.equal(evidence?.schema,1,'raw resource evidence required');assert.equal(evidence.lost,false,'resource recorder loss');
 assert.equal(evidence.samplerIntervalMs,1000,'resource sampler interval incompatible');assert.deepEqual(evidence.coverage,resourceCoverage,'resource coverage incompatible');
 const required=phaseContracts[evidence.contract];assert.ok(required,'resource phase contract required');
 assert.ok(Array.isArray(evidence.native)&&evidence.native.length>0,'raw native resource records required');
 for(const row of evidence.native)if(row.resource?.kind==='sample'&&row.resource.boundary!=='interval')assert.ok(required.includes(row.resource.phase),'unexpected phase boundary');
 const sequences=new Set();for(const row of evidence.native){number(row.seq,'sequence');number(row.ns,'monotonic time');assert.ok(!sequences.has(row.seq),'duplicate resource sequence');sequences.add(row.seq);assert.equal(row.stage,'resource');}
 // Recorder arrival order is not clock order. Cumulative counters must never
 // decrease anywhere within this process, including unscoped/inter-phase samples.
 const cumulative=evidence.native.filter(row=>row.resource?.kind==='sample').sort((a,b)=>a.ns-b.ns||a.seq-b.seq);let previous;
 for(const row of cumulative){for(const lane of ['metadata','usage','index','links']){const value=number(row.resource.readBytes?.[lane],`${lane} cumulative sample`);if(previous)assert.ok(value>=previous.resource.readBytes[lane],`${lane} cumulative counter reset`);}previous=row;}
 const phases={};
 for(const phase of required){
  const boundaries=evidence.native.filter(row=>row.resource?.kind==='sample'&&row.resource.phase===phase&&row.resource.boundary!=='interval');
  assert.equal(boundaries.length,2,`${phase}: exact start/end evidence required`);
  const start=boundaries.find(row=>row.resource.boundary==='start'),end=boundaries.find(row=>row.resource.boundary==='end');
  assert.ok(start&&end&&end.ns>start.ns,`${phase}: ordered boundaries required`);
  const rows=evidence.native.filter(row=>row.ns>=start.ns&&row.ns<=end.ns);
  const samples=rows.filter(row=>row.resource.kind==='sample').map(row=>row.resource);
  const rss=samples.map(row=>measured(row.nativeRssInclusive,'native RSS'));
  const db=samples.map(row=>measured(row.db,'DB'));
  const wal=samples.map(row=>measured(row.walRetained,'WAL',{absent:true}));
  const readBytes={};for(const lane of ['metadata','usage','index','links']){readBytes[lane]=number(end.resource.readBytes?.[lane],lane)-number(start.resource.readBytes?.[lane],lane);number(readBytes[lane],lane);}
  const orderedSamples=rows.filter(row=>row.resource.kind==='sample').sort((a,b)=>a.ns-b.ns);
  for(let i=1;i<orderedSamples.length;i++)assert.ok(orderedSamples[i].ns-orderedSamples[i-1].ns<=2_500_000_000,`${phase}: sampler gap exceeds 2.5 intervals`);
  const transactions=rows.filter(row=>row.resource.kind==='transaction').map(row=>row.resource);
  const mutations={};let waitNs=0,holdNs=0;
  for(const tx of transactions){
   assert.ok(['committed','rolled_back','begin_failed','commit_failed','rollback_failed'].includes(tx.outcome),'transaction outcome');
   assert.ok(resourceCoverage.transactions.includes(`${tx.operation}:${tx.transactionMode}`),'transaction mode/coverage incompatible');number(tx.waitNs,'writer wait');waitNs=Math.max(waitNs,tx.waitNs);if(tx.outcome!=='begin_failed'){number(tx.holdNs,'transaction hold');holdNs=Math.max(holdNs,tx.holdNs);}
   assert.ok(tx.mutations&&typeof tx.mutations==='object');
   if(tx.outcome!=='committed')assert.deepEqual(tx.mutations,{},'rollback must discard callbacks');
   for(const [table,counts] of Object.entries(tx.mutations)){assert.equal(counts.length,3);const totals=mutations[table]??=[0,0,0];counts.forEach((v,i)=>totals[i]+=number(v,'mutation'));}
  }
  const windowNs=Math.min(5_000_000_000,Math.floor((end.ns-start.ns)/2));
  const firstWindow=orderedSamples.filter(row=>row.ns<=start.ns+windowNs).map(row=>row.resource.nativeRssInclusive.bytes);
  const lastWindow=orderedSamples.filter(row=>row.ns>=end.ns-windowNs).map(row=>row.resource.nativeRssInclusive.bytes);
  phases[phase]={nativeRssWindowMedianDelta:median(lastWindow)-median(firstWindow),nativeRssWindowMs:windowNs/1e6,writerWaitMedianNs:transactions.length?median(transactions.map(tx=>tx.waitNs)):null,transactionHoldMedianNs:transactions.some(tx=>tx.holdNs!==null)?median(transactions.filter(tx=>tx.holdNs!==null).map(tx=>tx.holdNs)):null,durationMs:(end.ns-start.ns)/1e6,readBytes,mutations,nativeRssPeak:Math.max(...rss),dbPeak:Math.max(...db),walRetainedPeak:wal.every(v=>v===null)?null:Math.max(...wal.filter(v=>v!==null)),walAbsentSamples:wal.filter(v=>v===null).length,writerWaitMaxNs:waitNs,transactionHoldMaxNs:holdNs,transactions:transactions.length,samples:samples.length};
 }
 validateBrowserEvidence(evidence,required);
 for(const phase of required){phases[phase].rendererJsHeapInclusive={};for(const role of ['desktop','paired']){const rows=evidence.browser.filter(row=>row.role===role&&row.phase===phase);assert.ok(rows.length>0,`${phase}/${role}: browser phase samples required`);if(engine==='chromium')phases[phase].rendererJsHeapInclusive[role]={status:'measured',bytes:Math.max(...rows.map(row=>measured(row.heap,'Chromium phase heap')))};else {for(const row of rows){assert.equal(row.heap.status,'unknown');assert.equal(typeof row.heap.reason,'string');}phases[phase].rendererJsHeapInclusive[role]={status:'unknown',reason:'engine_unavailable'};}}}
 const browserHeap={};for(const role of ['desktop','paired']){
  const rows=evidence.browser.filter(row=>row.role===role);assert.ok(rows.length>0,`${role}: browser capability evidence required`);
  if(engine==='chromium')browserHeap[role]={status:'measured',bytes:Math.max(...rows.map(row=>measured(row.heap,'Chromium telemetry-inclusive heap')))};
  else {assert.equal(engine,'webkit');for(const row of rows){assert.equal(row.heap.status,'unknown');assert.equal(typeof row.heap.reason,'string');assert.equal(row.heap.bytes,undefined);}browserHeap[role]={status:'unknown',reason:'engine_unavailable'};}
 }
 return {phases,browserHeap,applicationOnlyMemory:{status:'unknown',reason:'telemetry_not_separable'},browserProcessMemory:{status:'unknown',reason:'unsupported'}};
}
export function validateResourceResult(result){
 assert.ok(result.resources?.evidence,'raw phase evidence required');
 assert.equal(result.resources.evidence.contract,result.mode==='mixed'?'mixed':'startup','applicable resource phase contract');
 if(result.resources.evidence.schema===2){assert.equal(result.mode,'mixed');assert.equal(result.warm,result.resources.evidence.segments[0].warm,'initial cold/warm identity');assert.deepEqual(result.resources.summary,summarizeMixedResources(result.resources.evidence,result.engine),'segmented resource summary must reconcile raw phases');return result.resources;}
 assert.deepEqual(result.resources.summary,summarizeResources(result.resources.evidence,result.engine),'resource summary must reconcile raw phases');
 return result.resources;
}
// These are raw browser observations captured at the same boundary as the summary,
// not copies of result.samples. The action milestones are absolute renderer times.
export function startupMeasurements(raw){
 assert.ok(raw&&raw.measurement?.lost===false,'raw browser evidence missing or lost');
 const finite=(v,label)=>{assert.ok(Number.isFinite(v)&&v>=0,`raw ${label} required`);return v;};
 const duration=key=>{const elapsed=finite(raw.detail?.[key],key)-finite(raw.detail?.start,'detail start');assert.ok(elapsed>=0);return elapsed;};
 assert.ok(Array.isArray(raw.commits)&&raw.commits.length>0,'raw profiling commits required');
 assert.ok(raw.longTasks===null||Array.isArray(raw.longTasks),'raw long task capability required');
 return {firstUsefulQueue:finite(raw.firstUsefulQueue,'queue'),fullQueue:finite(raw.fullQueue,'full queue'),firstUsefulDetail:duration('visible'),draftReadyMs:duration('draft'),freshActionReadyMs:duration('action'),uiCommits:raw.commits.length,uiCommitMs:raw.commits.reduce((n,c)=>n+finite(c.actual,'commit duration'),0),longTaskCount:raw.longTasks?.length??null,longTaskMs:raw.longTasks?.reduce((n,t)=>n+finite(t.duration,'long task duration'),0)??null};
}
export function validateStartupEvidence(result){
 validateResourceResult(result);
 assert.deepEqual(Object.keys(result.resources.startup??{}).sort(),['desktop','paired'],'exact raw startup roles required');
 for(const sample of result.samples){const expected=startupMeasurements(result.resources.startup[sample.role]);for(const [key,value] of Object.entries(expected))assert.equal(sample[key],value,`${sample.role} ${key} must reconcile raw startup phase`);}
}
export async function observeBrowserResources(pages,engine){
 const records=[],phases=[];let lost=false,pending=Promise.resolve(),timer,active;
 const sessions=engine==='chromium'?await Promise.all(pages.map(async page=>{const session=await page.context().newCDPSession(page);await session.send('Performance.enable');return session;})):[];
 const sample=(boundary,phase)=>{pending=pending.then(async()=>{for(const [i] of pages.entries()){
  if(records.length>=100000){lost=true;return;}
  let heap={status:'unknown',reason:'engine_unavailable'};
  if(engine==='chromium'){try{const data=await sessions[i].send('Performance.getMetrics');const bytes=data.metrics.find(m=>m.name==='JSHeapUsedSize')?.value;heap=Number.isSafeInteger(bytes)?{status:'measured',bytes}:{status:'unknown',reason:'metric_unavailable'};}catch{heap={status:'unknown',reason:'probe_failed'};}}
  records.push({seq:records.length+1,at:performance.now(),role:i?'paired':'desktop',phase,boundary,heap});
 }});return pending;};
 const boundary=async(phase,name,control)=>{const beforeMs=performance.now();await control(`resource-${phase}-${name}`);return {beforeMs,afterMs:performance.now()};};
 return {
  async startPhase(phase,control){assert.equal(active,undefined,'browser phase already active');assert.ok(!phases.some(p=>p.phase===phase),'duplicate browser phase');active={phase,start:await boundary(phase,'start',control)};phases.push(active);await sample('start',phase);timer=setInterval(()=>void sample('interval',phase),1000);},
  async endPhase(phase,control){assert.equal(active?.phase,phase,'matching active browser phase required');clearInterval(timer);await pending;await sample('end',phase);active.end=await boundary(phase,'end',control);active=undefined;},
  async stop(){clearInterval(timer);await pending;for(const session of sessions)await session.detach();return {records,lost,clock:{clock:'node-performance-ms',phases}};},
 };
}
