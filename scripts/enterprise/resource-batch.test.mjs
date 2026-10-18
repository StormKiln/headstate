import assert from 'node:assert/strict';import test from 'node:test';
import {resourceBatchPlan,deriveResourceLimits,compareObserverPair} from './resource-batch.mjs';
test('resource batches reserve independent cold/warm validation and alternating A/B pairs',()=>{
 for(const stage of ['calibrate','validate']){const plan=resourceBatchPlan(stage);assert.equal(plan.length,10);assert.equal(plan.filter(r=>r.warm).length,5);}
 const pairs=resourceBatchPlan('overhead');assert.equal(pairs.length,10);assert.deepEqual(pairs.slice(0,4).map(p=>p.arm),['disabled','enabled','enabled','disabled']);
 assert.throws(()=>deriveResourceLimits([]),/five cold/);
 assert.throws(()=>compareObserverPair({},{}));
});
import {fixture} from './resource-fixtures.mjs';
import {summarizeResources,phaseContracts} from './resources.mjs';
import {defaultWorkload} from './workload.mjs';
import {validateResourceLimits} from './resource-batch.mjs';
function mixed(warm=false){
 const run=fixture('chromium');const template=run.resources.evidence.native;run.resources.evidence.contract='mixed';run.resources.evidence.native=phaseContracts.mixed.flatMap((phase,i)=>template.map((row,j)=>({...structuredClone(row),seq:2*i+j+1,ns:(2*i+j+1)*1e9,resource:{...structuredClone(row.resource),phase}})));run.resources.evidence.browser=phaseContracts.mixed.flatMap(phase=>run.resources.evidence.browser.map(row=>({...row,phase})));run.resources.summary=summarizeResources(run.resources.evidence,'chromium');
 return {...run,pass:true,mode:'mixed',warm,workload:{...structuredClone(defaultWorkload),scenario:'mixed'},environment:{platform:'darwin',architecture:'arm64',osRelease:'25.6.0',cpu:'fixture CPU',build:'debug',strictMode:false,browser:{engine:'chromium',version:'140.0'},sourceRevision:'before'},resourceIdentity:{manifestSha256:'fixture',toolchain:'fixture',phasePlan:Object.fromEntries(phaseContracts.mixed.map(p=>[p,1000]))},parentObservation:{lost:false,wallMs:1000,nativeRssPeak:100,artifactBytes:100,commandLatencyMaxMs:3},nativeBinarySha256:'same',mixedOutcome:{rows:150,corpusComplete:true}};
}
test('resource calibration freezes comparable observations; independent validation rejects exceeded or incompatible evidence',()=>{
 const runs=resourceBatchPlan('calibrate').map(item=>mixed(item.warm));const policy=deriveResourceLimits(runs);const after=mixed();after.environment.sourceRevision='after';validateResourceLimits(after,policy);
 for(const mutate of [r=>r.resources.evidence.lost=true,r=>r.resourceIdentity.manifestSha256='other',r=>r.environment.cpu='other',r=>r.workload.populations.reviewing=250,r=>r.parentObservation.commandLatencyMaxMs=10000]){const bad=mixed();mutate(bad);assert.throws(()=>validateResourceLimits(bad,policy));}
});
test('observer pairs require identical binary and actual product outcomes with independent measures',()=>{
 const enabled=mixed(),disabled={...mixed(),resources:{enabled:false}};assert.equal(compareObserverPair(disabled,enabled).deltas.wallMs,0);
 for(const mutate of [r=>r.nativeBinarySha256='other',r=>delete r.mixedOutcome,r=>r.mixedOutcome.rows=149,r=>r.parentObservation.nativeRssPeak=null]){const bad=structuredClone(disabled);mutate(bad);assert.throws(()=>compareObserverPair(bad,enabled));}
});
