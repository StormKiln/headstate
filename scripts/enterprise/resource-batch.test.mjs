import assert from 'node:assert/strict';import test from 'node:test';
import {resourceBatchPlan,deriveResourceLimits,compareObserverPair} from './resource-batch.mjs';
test('resource batches reserve independent cold/warm validation and alternating A/B pairs',()=>{
 for(const stage of ['calibrate','validate']){const plan=resourceBatchPlan(stage);assert.equal(plan.length,10);assert.equal(plan.filter(r=>r.warm).length,5);}
 const pairs=resourceBatchPlan('overhead');assert.equal(pairs.length,10);assert.deepEqual(pairs.slice(0,4).map(p=>p.arm),['disabled','enabled','enabled','disabled']);
 assert.throws(()=>deriveResourceLimits([]),/five cold/);
 assert.throws(()=>compareObserverPair({},{}));
});
import {fixture} from './resource-fixtures.mjs';
import {phaseContracts} from './resources.mjs';
import {defaultWorkload} from './workload.mjs';
import {validateResourceLimits,assertResourceComparable} from './resource-batch.mjs';
import {summarizeMixedResources} from './mixed-resources.mjs';
function mixed(warm=false){
 const processes=[{pid:100,nonce:'a'},{pid:101,nonce:'b'}];const segmentPhasePlan={initial:Object.fromEntries(phaseContracts.mixed_initial.map(p=>[p,4000])),resumed:{churn_restart:4000}};
 const evidence={schema:2,contract:'mixed',segments:['initial','resumed'].map((kind,i)=>({kind,warm:i===1||warm,process:processes[i],evidence:fixture('chromium',`mixed_${kind}`).resources.evidence}))};
 const run={engine:'chromium',resources:{enabled:true,evidence,summary:summarizeMixedResources(evidence,'chromium')},calibrationEligible:true,restart:{actualProcesses:processes,sourceResultSha256:'a'.repeat(64),resumedResultSha256:'b'.repeat(64),checkpointSha256:'c'.repeat(64)}};
 return {...run,pass:true,mode:'mixed',warm,workload:{...structuredClone(defaultWorkload),scenario:'mixed'},environment:{platform:'darwin',architecture:'arm64',osRelease:'25.6.0',cpu:'fixture CPU',build:'debug',strictMode:false,browser:{engine:'chromium',version:'140.0'},sourceRevision:'before'},resourceIdentity:{manifestSha256:'fixture',toolchain:'fixture',phasePlanStatus:'fixed',segmentPhasePlan,phasePlan:Object.fromEntries(phaseContracts.mixed.map(p=>[p,p==='churn_restart'?8000:4000]))},parentObservation:{lost:false,wallMs:1000,nativeRssPeak:100,artifactBytes:100,commandLatencyMaxMs:3},nativeBinarySha256:'same',mixedOutcome:{rows:150,corpusComplete:true}};
}
test('resource calibration freezes comparable observations; independent validation rejects exceeded or incompatible evidence',()=>{
 const runs=resourceBatchPlan('calibrate').map(item=>mixed(item.warm));const policy=deriveResourceLimits(runs);const after=mixed();after.environment.sourceRevision='after';validateResourceLimits(after,policy);
 for(const mutate of [r=>r.resources.evidence.segments[0].evidence.lost=true,r=>r.resourceIdentity.manifestSha256='other',r=>r.environment.cpu='other',r=>r.workload.populations.reviewing=250,r=>r.parentObservation.commandLatencyMaxMs=10000]){const bad=mixed();mutate(bad);assert.throws(()=>validateResourceLimits(bad,policy));}
});
test('observer pairs require identical binary and actual product outcomes with independent measures',()=>{
 const enabled=mixed(),disabled={...mixed(),resources:{enabled:false}};assert.equal(compareObserverPair(disabled,enabled).deltas.wallMs,0);
 for(const mutate of [r=>r.nativeBinarySha256='other',r=>delete r.mixedOutcome,r=>r.mixedOutcome.rows=149,r=>r.parentObservation.nativeRssPeak=null]){const bad=structuredClone(disabled);mutate(bad);assert.throws(()=>compareObserverPair(bad,enabled));}
});

import {resourceBatchInvocation} from './resource-batch.mjs';
test('each warm sample requests independent baseline preparation instead of inheriting a mutated cold profile',()=>{
 const warm=resourceBatchInvocation({warm:true},'/retained/warm');
 assert.deepEqual(warm.args,['scripts/enterprise-run.mjs','/retained/warm','chromium','mixed']);
 assert.equal(warm.env.ENTERPRISE_MIXED_WARM,'1');
 assert.equal(resourceBatchInvocation({warm:false,arm:'disabled'},'/retained/cold').env.ENTERPRISE_RESOURCE_DISABLED,'1');
});

test('calibration rejects exploratory and single-process mixed evidence',()=>{
 for(const mutate of [r=>r.calibrationEligible=false,r=>delete r.restart,r=>r.restart.actualProcesses[1]=r.restart.actualProcesses[0],r=>r.resources.evidence.schema=1,r=>r.resourceIdentity.segmentPhasePlan=null]){const bad=mixed();mutate(bad);assert.throws(()=>assertResourceComparable(bad,bad));}
});

test('frozen write budgets cover fixed callback keys and reject extra or unclassified writes',()=>{
 const runs=resourceBatchPlan('calibrate').map(item=>mixed(item.warm)),policy=deriveResourceLimits(runs);
 assert.equal(policy.limits.cold['active_ingestion.mutations.claude_session.insert'],0);
 assert.equal(policy.limits.cold['active_ingestion.transactions'],0);
 const mutate=(table)=>{const run=mixed(),e=run.resources.evidence.segments[0].evidence,n=e.native.find(r=>r.resource.phase==='active_ingestion');e.native.push({seq:e.native.length+1,ns:n.ns+1000000,stage:'resource',operation:'resource',id:0,resource:{kind:'transaction',phase:'active_ingestion',operation:'corpus-metadata',transactionMode:'immediate',outcome:'committed',waitNs:0,holdNs:1,mutations:{[table]:[1,0,0]}}});e.native.sort((a,b)=>a.ns-b.ns);e.native.forEach((r,i)=>r.seq=i+1);for(const phase of e.browserClock.phases){phase.start.nativeSeq=e.native.find(r=>r.ns===phase.start.nativeNs).seq;phase.end.nativeSeq=e.native.find(r=>r.ns===phase.end.nativeNs).seq;}run.resources.summary=summarizeMixedResources(run.resources.evidence,'chromium');return run;};
 for(const table of ['claude_session','unclassified_table','new_table'])assert.throws(()=>validateResourceLimits(mutate(table),policy));
});

import {readFileSync} from 'node:fs';import {mutationBudgetTables} from './resource-batch.mjs';
test('write budget table keys exactly cover the native scoped callback allowlist',()=>{
 const source=readFileSync(new URL('../../src-tauri/src/enterprise_harness/resources.rs',import.meta.url),'utf8').split('fn table_name')[1].split('type Mutations')[0];
 assert.deepEqual(mutationBudgetTables,[...source.matchAll(/"([a-z_]+)" =>/g)].map(match=>match[1]));
});

import {validateFrozenResourcePolicy} from './resource-batch.mjs';
test('static frozen policy admission rejects malformed descriptors, unknown keys and absent/non-numeric limits',()=>{
 const policy=deriveResourceLimits(resourceBatchPlan('calibrate').map(item=>mixed(item.warm)));validateFrozenResourcePolicy(policy);
 for(const mutate of [p=>delete p.sourceRevisions,p=>p.headroom=null,p=>p.coverage={},p=>p.workload.populations.reviewing=250,p=>p.environment.browser.version=null,p=>p.resourceIdentity.toolchain='',p=>delete p.limits.warm,p=>delete p.limits.cold['idle.transactions'],p=>p.limits.cold.commandLatencyMaxMs=NaN,p=>p.limits.cold.commandLatencyMaxMs=-1,p=>p.limits.cold.extra=2,p=>p.limits.warm={}]){const bad=structuredClone(policy);mutate(bad);assert.throws(()=>validateFrozenResourcePolicy(bad));}
});
