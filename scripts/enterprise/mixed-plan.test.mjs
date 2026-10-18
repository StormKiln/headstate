import test from 'node:test';import assert from 'node:assert/strict';
import {readFile,mkdtemp,writeFile,access} from 'node:fs/promises';import {tmpdir} from 'node:os';import {resolve} from 'node:path';import {spawnSync} from 'node:child_process';
import {validateMixedPhasePlan} from './mixed-plan.mjs';
async function spawnGuard(root){const marker=resolve(root,'spawned');const preload=resolve(root,'guard.mjs');await writeFile(preload,`import cp from 'node:child_process';import {syncBuiltinESMExports} from 'node:module';import {writeFileSync} from 'node:fs';cp.spawn=()=>{writeFileSync(${JSON.stringify(marker)},'spawned');throw Error('unexpected child launch');};syncBuiltinESMExports();`);return {marker,args:['--import',preload]};}
const fixed=JSON.parse(await readFile(new URL('./mixed-phase-plan-10.9.json',import.meta.url),'utf8'));
test('approved fixed descriptor validates, malformed or unbounded phase plans reject',()=>{
 assert.equal(validateMixedPhasePlan(fixed),fixed);
 for(const mutate of [p=>p.schema=2,p=>p.extra=1,p=>delete p.segments.resumed,p=>p.segments.initial.extra=5000,p=>p.segments.initial.idle=4999,p=>p.segments.initial.idle=Infinity,p=>p.segments.initial.idle=5000.5,p=>p.segments.initial.idle=3600001]){const p=structuredClone(fixed);mutate(p);assert.throws(()=>validateMixedPhasePlan(p));}
});
test('all batch stages reject absent/invalid descriptor before output setup or child launch',async()=>{
 const root=await mkdtemp(resolve(tmpdir(),'r109-batch-preflight-'));const guard=await spawnGuard(root);const invalid=resolve(root,'invalid.json');await writeFile(invalid,'{}');
 for(const stage of ['calibrate','validate','overhead'])for(const path of [undefined,invalid]){
  const out=resolve(root,`${stage}-${path?'invalid':'missing'}`);const env={...process.env};delete env.ENTERPRISE_MIXED_PHASE_PLAN;if(path)env.ENTERPRISE_MIXED_PHASE_PLAN=path;
  const run=spawnSync(process.execPath,[...guard.args,'scripts/enterprise-resources.mjs',out,stage],{env,encoding:'utf8',timeout:5000});assert.equal(run.status,1);assert.equal(run.signal,null);assert.match(run.stderr,/PHASE_PLAN required|mixed phase schema/);await assert.rejects(access(out));await assert.rejects(access(guard.marker));
 }
});
test('validation rejects a mismatched frozen descriptor before output or child launch',async()=>{
 const root=await mkdtemp(resolve(tmpdir(),'r109-batch-policy-'));const guard=await spawnGuard(root);const policy=resolve(root,'limits.json');await writeFile(policy,JSON.stringify({schema:1,stage:'frozen',resourceIdentity:{segmentPhasePlan:{}}}));const out=resolve(root,'out');
 const run=spawnSync(process.execPath,[...guard.args,'scripts/enterprise-resources.mjs',out,'validate',policy],{env:{...process.env,ENTERPRISE_MIXED_PHASE_PLAN:resolve('scripts/enterprise/mixed-phase-plan-10.9.json')},encoding:'utf8',timeout:5000});assert.equal(run.status,1);assert.match(run.stderr,/frozen phase descriptor/);await assert.rejects(access(out));await assert.rejects(access(guard.marker));
});

test('matching phase plan with malformed frozen coverage rejects without launching a sample',async()=>{
 const root=await mkdtemp(resolve(tmpdir(),'r109-batch-malformed-policy-')),guard=await spawnGuard(root),out=resolve(root,'out'),policy=resolve(root,'limits.json');
 await writeFile(policy,JSON.stringify({schema:1,stage:'frozen',resourceIdentity:{segmentPhasePlan:fixed.segments},coverage:{}}));
 const run=spawnSync(process.execPath,[...guard.args,'scripts/enterprise-resources.mjs',out,'validate',policy],{env:{...process.env,ENTERPRISE_MIXED_PHASE_PLAN:resolve('scripts/enterprise/mixed-phase-plan-10.9.json')},encoding:'utf8',timeout:5000});
 assert.equal(run.status,1);assert.doesNotMatch(run.stderr,/unexpected child launch/);await assert.rejects(access(out));await assert.rejects(access(guard.marker));
});
