import assert from 'node:assert/strict';
import {summarizeResources} from './resources.mjs';
// Process clocks/counters remain local. Restart authority is separately validated
// against immutable native/profile/oracle artifacts before result admission.
export function summarizeMixedResources(evidence,engine){
 assert.equal(evidence.schema,2);assert.equal(evidence.contract,'mixed');assert.deepEqual(evidence.segments.map(s=>s.kind),['initial','resumed'],'exact ordered mixed process segments required');
 const processes=new Set();const segments=evidence.segments.map(segment=>{
  assert.ok(Number.isSafeInteger(segment.process?.pid)&&segment.process.pid>0,'native process PID required');assert.ok(typeof segment.process.nonce==='string'&&segment.process.nonce.length>0,'native handshake identity required');assert.ok(!processes.has(segment.process.nonce),'distinct process handshake required');processes.add(segment.process.nonce);
  assert.equal(typeof segment.warm,'boolean');if(segment.kind==='resumed')assert.equal(segment.warm,true,'resumed process is warm');
  assert.equal(segment.evidence.contract,`mixed_${segment.kind}`,'local phase contract must match segment');
  return {kind:segment.kind,process:segment.process,warm:segment.warm,summary:summarizeResources(segment.evidence,engine)};
 });
 assert.notEqual(segments[0].process.pid,segments[1].process.pid,'actual new native PID required');
 const phases={};
 for(const phase of ['idle','unchanged_refresh','active_ingestion','churn_restart']){
  const parts=segments.filter(s=>s.summary.phases[phase]).map(s=>({kind:s.kind,values:s.summary.phases[phase]}));assert.ok(parts.length>0);
  const sum=key=>parts.reduce((n,p)=>n+p.values[key],0),max=key=>Math.max(...parts.map(p=>p.values[key]));
  const mutations={},readBytes={};for(const {values} of parts){for(const [lane,bytes] of Object.entries(values.readBytes))readBytes[lane]=(readBytes[lane]??0)+bytes;for(const [table,counts] of Object.entries(values.mutations)){const total=mutations[table]??=[0,0,0];counts.forEach((n,i)=>total[i]+=n);}}
  const wal=parts.map(p=>p.values.walRetainedPeak).filter(n=>n!==null),rendererJsHeapInclusive={};for(const role of ['desktop','paired'])rendererJsHeapInclusive[role]=engine==='chromium'?{status:'measured',bytes:Math.max(...parts.map(p=>p.values.rendererJsHeapInclusive[role].bytes))}:{status:'unknown',reason:'engine_unavailable'};
  phases[phase]={durationMs:sum('durationMs'),readBytes,mutations,nativeRssPeak:max('nativeRssPeak'),dbPeak:max('dbPeak'),walRetainedPeak:wal.length?Math.max(...wal):null,walAbsentSamples:sum('walAbsentSamples'),writerWaitMaxNs:max('writerWaitMaxNs'),transactionHoldMaxNs:max('transactionHoldMaxNs'),transactions:sum('transactions'),samples:sum('samples'),rendererJsHeapInclusive,nativeRssWindowMedianDelta:null,nativeRssWindowMs:null,writerWaitMedianNs:null,transactionHoldMedianNs:null,segmentMemoryDrift:parts.map(({kind,values})=>({segment:kind,nativeRssWindowMedianDelta:values.nativeRssWindowMedianDelta,nativeRssWindowMs:values.nativeRssWindowMs}))};
 }
 return {phases,segments,applicationOnlyMemory:{status:'unknown',reason:'telemetry_not_separable'},browserProcessMemory:{status:'unknown',reason:'unsupported'},clockSemantics:'per-process only; summed active duration excludes shutdown/restart downtime'};
}
