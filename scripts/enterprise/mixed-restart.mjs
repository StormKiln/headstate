import assert from 'node:assert/strict';
import {readFile,writeFile,cp,mkdir} from 'node:fs/promises';
import {resolve} from 'node:path';
import {createChurnPlan,initialOracle,advanceOracle,digest} from './churn-plan.mjs';
import {assertMixedCorpus} from './mixed-observation.mjs';
import {profileHashes,assertProviderMatchesOracle} from './churn-restart.mjs';
export function createMixedPlan(workload){
 assert.equal(workload.scenario,'mixed');assert.equal(workload.scheduleVersion,'mixed-v1');
 const seeded=createChurnPlan({...workload,scheduleVersion:'churn-v3'});
 return {schema:1,version:'mixed-v1',workload,initial:seeded.initial,events:seeded.events.slice(0,4),checkpointRevision:3};
}
export function validateMixedCheckpoint(record,source,expected){
 assert.equal(record.schema,1);assert.equal(record.kind,'mixed-checkpoint');
 assert.equal(source.pass,true,'source must pass');assert.equal(source.shutdown.clean,true,'clean source required');assert.equal(source.shutdown.telemetryLost,false);assert.equal(source.exit.code,0);
 assert.equal(record.sourceSha256,expected.sourceSha256,'source artifact hash');assert.equal(record.sourceSha256,digest(source),'source content hash');
 assert.equal(record.profile,expected.profile,'root-bound profile must remain at the exact same path');
 assert.deepEqual(record.plan,createMixedPlan(expected.plan.workload));assert.deepEqual(record.plan,expected.plan);
 for(const [field,key] of [['plan','plan'],['oracle','oracle'],['manifest','manifest'],['profileHashes','profile'],['corpus','corpus']])assert.equal(digest(record[field]),record.hashes[key],`${field} hash`);
 assert.deepEqual(record.manifest,expected.manifest,'exact generated source manifest');assert.deepEqual(record.provenance,expected.provenance,'native/UI/harness provenance');assert.deepEqual(source.provenance,record.provenance);
 assert.deepEqual(record.profileHashes,expected.profileHashes,'profile byte identity before new process opens');
 assert.equal(source.mixed.kind,'initial');assert.equal(source.mixed.planHash,digest(record.plan));assert.deepEqual(source.mixed.oracle,record.oracle);assert.deepEqual(source.mixed.receipts,record.receipts);assert.deepEqual(source.mixed.corpus,record.corpus);
 let oracle=initialOracle(record.plan);for(const event of record.plan.events.slice(0,record.plan.checkpointRevision))oracle=advanceOracle(oracle,event,record.receipts.find(r=>r.revision===event.revision)?.receipt);
 assert.deepEqual(record.oracle,oracle,'independent source oracle replay');assert.equal(record.corpus.stored.paused,true,'durable pause must precede restart');
 assert.deepEqual(record.receipts.map(r=>r.revision),[2],'exact initial delivered approval obligation');return record;
}
export async function saveMixedCheckpoint({out,profile,provider,plan,manifest,source}){
 assert.equal(source.pass,true);assert.equal(source.shutdown.clean,true);assert.equal(source.exit.code,0);
 const hashes=await profileHashes(profile),savedProvider=JSON.parse(JSON.stringify(provider.data,(key,value)=>key==='pullRequest'?undefined:value));assertProviderMatchesOracle(savedProvider,source.mixed.oracle);
 const record={schema:1,kind:'mixed-checkpoint',profile:resolve(profile),plan,oracle:source.mixed.oracle,receipts:source.mixed.receipts,manifest,profileHashes:hashes,provenance:source.provenance,corpus:source.mixed.corpus,provider:savedProvider,sourceSha256:digest(source)};
 record.hashes={plan:digest(plan),oracle:digest(record.oracle),manifest:digest(manifest),profile:digest(hashes),corpus:digest(record.corpus),provider:digest(savedProvider)};
 await mkdir(resolve(out,'checkpoint'),{recursive:false});await cp(profile,resolve(out,'checkpoint/profile'),{recursive:true,errorOnExist:true,force:false});
 assert.deepEqual(await profileHashes(resolve(out,'checkpoint/profile')),hashes,'immutable checkpoint snapshot byte identity');
 await writeFile(resolve(out,'mixed-checkpoint.json'),JSON.stringify(record,null,2));return record;
}
export async function loadMixedCheckpoint({sourceDir,profile,provider,plan,manifest,provenance}){
 const source=JSON.parse(await readFile(resolve(sourceDir,'result.json'),'utf8')),record=JSON.parse(await readFile(resolve(sourceDir,'mixed-checkpoint.json'),'utf8'));
 validateMixedCheckpoint(record,source,{profile:resolve(profile),plan,manifest,provenance,profileHashes:await profileHashes(profile),sourceSha256:digest(source)});
 assert.deepEqual(await profileHashes(resolve(sourceDir,'checkpoint/profile')),record.profileHashes,'retained immutable checkpoint snapshot');
 assert.equal(digest(record.provider),record.hashes.provider);assertProviderMatchesOracle(record.provider,record.oracle);
 const data=structuredClone(record.provider);for(const row of data.rows){row.repository=data.repos.find(repo=>repo.nameWithOwner===row.repository.nameWithOwner);row.headRepository=row.repository;}Object.assign(provider.data,data);
 assert.equal(typeof source.warm,'boolean');return {...record,initialWarm:source.warm};
}

export async function saveMixedPreparation({out,profile,provider,plan,manifest,source}){
 assert.equal(source.pass,true);assert.equal(source.shutdown.clean,true);assert.equal(source.exit.code,0);assert.equal(source.mixed.kind,'preparation');assert.deepEqual(source.mixed.oracle,initialOracle(plan));assert.deepEqual(source.mixed.receipts,[]);assertMixedCorpus(source.mixed.corpus,manifest,{complete:true});assert.equal(source.mixed.corpus.stored.paused,false);assertProviderMatchesOracle(provider.data,source.mixed.oracle);
 const hashes=await profileHashes(profile),record={schema:1,kind:'mixed-preparation',profile:resolve(profile),plan,manifest,oracle:source.mixed.oracle,receipts:[],corpus:source.mixed.corpus,provenance:source.provenance,profileHashes:hashes,sourceSha256:digest(source)};
 await mkdir(resolve(out,'checkpoint'),{recursive:false});await cp(profile,resolve(out,'checkpoint/profile'),{recursive:true,errorOnExist:true,force:false});assert.deepEqual(await profileHashes(resolve(out,'checkpoint/profile')),hashes);await writeFile(resolve(out,'mixed-preparation.json'),JSON.stringify(record,null,2));return record;
}
export async function loadMixedPreparation({sourceDir,profile,plan,manifest,provenance}){
 const source=JSON.parse(await readFile(resolve(sourceDir,'result.json'),'utf8')),record=JSON.parse(await readFile(resolve(sourceDir,'mixed-preparation.json'),'utf8'));
 assert.equal(record.schema,1);assert.equal(record.kind,'mixed-preparation');assert.equal(source.pass,true);assert.equal(source.shutdown.clean,true);assert.equal(source.shutdown.telemetryLost,false);assert.equal(source.exit.code,0);assert.equal(record.profile,resolve(profile));assert.equal(record.sourceSha256,digest(source));assert.deepEqual(record.plan,plan);assert.deepEqual(plan,createMixedPlan(plan.workload));assert.deepEqual(record.manifest,manifest);assert.deepEqual(record.provenance,provenance);assert.deepEqual(source.provenance,provenance);assert.equal(source.mixed.kind,'preparation');assert.deepEqual(record.oracle,initialOracle(plan));assert.deepEqual(source.mixed.oracle,record.oracle);assert.deepEqual(record.receipts,[]);assert.deepEqual(source.mixed.receipts,[]);assert.deepEqual(record.corpus,source.mixed.corpus);assertMixedCorpus(record.corpus,manifest,{complete:true});assert.equal(record.corpus.stored.paused,false);
 assert.deepEqual(await profileHashes(profile),record.profileHashes,'warm preparation remains at exact original rooted profile');assert.deepEqual(await profileHashes(resolve(sourceDir,'checkpoint/profile')),record.profileHashes,'retained initial baseline snapshot');return record;
}
