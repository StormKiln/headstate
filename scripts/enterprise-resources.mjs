import {validateMixedPhasePlan} from './enterprise/mixed-plan.mjs';
// Mixed-workload batch foundation. The mixed adapter must produce the strict
// resourceIdentity/mixedOutcome witnesses; startup runs cannot stand in for it.
import assert from 'node:assert/strict';
import {mkdir,readFile,writeFile,readdir} from 'node:fs/promises';
import {spawn} from 'node:child_process';import {resolve} from 'node:path';
import {resourceBatchPlan,resourceBatchInvocation,deriveResourceLimits,validateResourceLimits,compareObserverPair,validateFrozenResourcePolicy} from './enterprise/resource-batch.mjs';
const root=resolve(process.argv[2]),stage=process.argv[3]??'calibrate';
const plan=resourceBatchPlan(stage);
assert.ok(process.env.ENTERPRISE_MIXED_PHASE_PLAN,'ENTERPRISE_MIXED_PHASE_PLAN required before any batch launch');
const phasePlan=validateMixedPhasePlan(JSON.parse(await readFile(process.env.ENTERPRISE_MIXED_PHASE_PLAN,'utf8')));
const policy=stage==='validate'?JSON.parse(await readFile(process.argv[4],'utf8')):undefined;
if(policy){assert.equal(policy.schema,1);assert.equal(policy.stage,'frozen');assert.deepEqual(policy.resourceIdentity.segmentPhasePlan,phasePlan.segments,'validation must use frozen phase descriptor');validateFrozenResourcePolicy(policy);}
await mkdir(root,{recursive:true});assert.equal((await readdir(root)).length,0,'retain previous batches; use a new output path');
await writeFile(resolve(root,'plan.json'),JSON.stringify({stage,plan,phasePlan,policy},null,2));
const runs=[];
for(const item of plan){
 const out=resolve(root,item.name);const invocation=resourceBatchInvocation(item,out);
 const code=await new Promise((done,reject)=>{const p=spawn(process.execPath,invocation.args,{stdio:'inherit',env:{...process.env,...invocation.env}});p.once('error',reject);p.once('exit',done);});
 const run=JSON.parse(await readFile(resolve(out,'result.json'),'utf8'));runs.push({...run,artifact:out,batch:item});await writeFile(resolve(root,'runs.json'),JSON.stringify(runs,null,2));assert.equal(code,0,'failed run retained');assert.equal(run.pass,true);
 if(policy)validateResourceLimits(run,policy);
}
if(stage==='calibrate')await writeFile(resolve(root,'limits.json'),JSON.stringify(deriveResourceLimits(runs),null,2));
if(stage==='overhead')await writeFile(resolve(root,'overhead.json'),JSON.stringify(Array.from({length:5},(_,i)=>compareObserverPair(runs.find(r=>r.batch.pair===i+1&&r.batch.arm==='disabled'),runs.find(r=>r.batch.pair===i+1&&r.batch.arm==='enabled'))),null,2));
