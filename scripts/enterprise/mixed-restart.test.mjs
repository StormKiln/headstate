import test from 'node:test';
import assert from 'node:assert/strict';
import {defaultWorkload} from './workload.mjs';
import {initialOracle,advanceOracle,digest} from './churn-plan.mjs';
import {createMixedPlan,validateMixedCheckpoint} from './mixed-restart.mjs';
function fixture(){
 const plan=createMixedPlan({...defaultWorkload,scenario:'mixed',scheduleVersion:'mixed-v1'});let oracle=initialOracle(plan);const receipts=[];
 for(const event of plan.events.slice(0,3)){const receipt=event.kind==='approve'?{ok:true,identity:event.identity,head:event.previousHead}:undefined;if(receipt)receipts.push({revision:event.revision,receipt});oracle=advanceOracle(oracle,event,receipt);}
 const provenance={nativeBinarySha256:'binary',browserAssets:[{path:'ui.js',sha256:'ui'}],harnessCode:[{path:'runner.mjs',sha256:'runner'}]},profileHashes={'.enterprise-synthetic':'marker','headstate.db':'db'},manifest={logicalSha256:'manifest',files:[{path:'synthetic/file.jsonl',sha256:'bytes'}]};
 const source={pass:true,shutdown:{clean:true,telemetryLost:false},exit:{code:0},process:{pid:100,nonce:'source'},mixed:{kind:'initial',oracle,receipts,corpus:{stored:{paused:true}},planHash:digest(plan)},provenance};
 const record={schema:1,kind:'mixed-checkpoint',profile:'/synthetic/stable-profile',plan,oracle,receipts,manifest,profileHashes,provenance,corpus:source.mixed.corpus,sourceSha256:digest(source)};
 record.hashes={plan:digest(plan),oracle:digest(oracle),manifest:digest(manifest),profile:digest(profileHashes),corpus:digest(record.corpus)};
 return {record,source,expected:{profile:record.profile,plan,manifest,provenance,profileHashes,sourceSha256:digest(source)}};
}
test('mixed checkpoint independently replays exact foreground obligations and linked source',()=>{const f=fixture();assert.equal(validateMixedCheckpoint(f.record,f.source,f.expected).oracle.revision,3);});
test('mixed checkpoint rejects failed, foreign, shifted-root and forged cached authority',()=>{
 for(const corrupt of [f=>f.source.pass=false,f=>f.source.shutdown.clean=false,f=>f.source.exit.code=2,f=>f.source.shutdown.telemetryLost=true,f=>f.record.profile='/other/profile',f=>f.record.oracle.rows[50].approved=false,f=>f.record.receipts[0].receipt.head='stale',f=>f.record.profileHashes['headstate.db']='changed',f=>f.record.provenance.nativeBinarySha256='other',f=>f.record.manifest.files[0].sha256='changed',f=>f.record.corpus.stored.paused=false]){const f=fixture();corrupt(f);assert.throws(()=>validateMixedCheckpoint(f.record,f.source,f.expected));}
});
