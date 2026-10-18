import assert from 'node:assert/strict';
import {mkdir,readdir,readFile,writeFile,stat} from 'node:fs/promises';
import {createWriteStream} from 'node:fs';
import {resolve} from 'node:path';
import {spawn} from 'node:child_process';
import {digest,initialOracle,advanceOracle,assertChurnFacts,assertChurnReady} from './churn-plan.mjs';
import {createMixedPlan} from './mixed-restart.mjs';
import {summarizeMixedResources} from './mixed-resources.mjs';
import {validateResourceResult} from './resources.mjs';
import {assertMixedCorpus} from './mixed-observation.mjs';
async function bytes(path){let total=0;for(const entry of await readdir(path,{withFileTypes:true})){const child=resolve(path,entry.name);if(entry.isDirectory())total+=await bytes(child);else if(entry.isFile())total+=(await stat(child)).size;}return total;}
export function validateMixedPair(initial,resumed){
 for(const run of [initial,resumed]){assert.equal(run.pass,true);assert.equal(run.shutdown.clean,true);assert.equal(run.shutdown.telemetryLost,false);assert.equal(run.exit.code,0);assert.equal(run.mixed.pass,true);assert.equal(run.statsRetirement.providerRequests,0);assert.equal(run.statsRetirement.commands,0);assert.equal(run.parentObservation.lost,false);}
 if(!initial.warm)assert.ok(initial.mixed.overlap?.length>0,'cold integration requires actual foreground/corpus overlap');
 assert.deepEqual(initial.provenance,resumed.provenance,'exact native/UI/harness continuity');assert.equal(initial.mixed.kind,'initial');assert.equal(resumed.mixed.kind,'resumed');assert.notEqual(initial.process.pid,resumed.process.pid,'actual different native PID');assert.notEqual(initial.process.nonce,resumed.process.nonce,'distinct native handshake');assert.equal(resumed.warm,true);
 const plan=createMixedPlan(initial.workload);assert.deepEqual(initial.workload,resumed.workload);assert.equal(initial.mixed.planHash,digest(plan));assert.equal(resumed.mixed.planHash,digest(plan));
 let oracle=initialOracle(plan);for(const event of plan.events){oracle=advanceOracle(oracle,event,resumed.mixed.receipts.find(r=>r.revision===event.revision)?.receipt);if(event.revision===plan.checkpointRevision)assert.deepEqual(initial.mixed.oracle,oracle);}
 assert.deepEqual(resumed.mixed.oracle,oracle);assert.deepEqual(initial.mixed.receipts.map(r=>r.revision),[2]);assert.deepEqual(resumed.mixed.receipts.map(r=>r.revision),[2,4]);assert.deepEqual(initial.mixed.receipts,resumed.mixed.receipts.slice(0,1));
 assert.equal(initial.mixed.corpus.stored.paused,true);assert.equal(resumed.mixed.pauseAfterRestart,true);assert.equal(resumed.mixed.cachedCorpus.before,0);assert.equal(resumed.mixed.cachedCorpus.after,0);assert.equal(resumed.mixed.corpus.stored.paused,false);
 for(const run of [initial,resumed]){assert.equal(run.mixed.finalClients.length,2,'both actual clients required');assert.deepEqual(run.mixed.consumerWitnesses.map(w=>w.role).sort(),['desktop','paired'],'both stored consumers required');for(const client of run.mixed.finalClients){assert.equal(client.lost,false);assertChurnFacts(run.mixed.oracle,client.facts);assertChurnReady(run.mixed.oracle,client.ready);}}return plan;
}
export async function runMixed({out,engine,warm,fixedPlanPath}){
 await mkdir(out,{recursive:true});assert.deepEqual(await readdir(out),[],'refuse existing mixed artifacts');
 const profile=resolve(out,'profile'),corpusRoot=resolve(profile,'generated-corpus/projects'),manifestPath=resolve(out,'mixed-fixture-manifest.json');
 await mkdir(corpusRoot,{recursive:true});await writeFile(resolve(profile,'.enterprise-synthetic'),'owned mixed generated profile\n',{flag:'wx'});
 const fixed=fixedPlanPath?JSON.parse(await readFile(fixedPlanPath,'utf8')):undefined;
 if(fixed){assert.equal(fixed.schema,1);assert.deepEqual(Object.keys(fixed.segments).sort(),['initial','resumed']);assert.deepEqual(Object.keys(fixed.segments.initial).sort(),['active_ingestion','churn_restart','idle','unchanged_refresh']);assert.deepEqual(Object.keys(fixed.segments.resumed),['churn_restart']);for(const phase of Object.values(fixed.segments).flatMap(Object.values))assert.ok(Number.isFinite(phase)&&phase>=5000);}
 const disabled=process.env.ENTERPRISE_RESOURCE_DISABLED==='1',segments=[],result={mode:'mixed',engine,warm,pass:false,calibrationEligible:!!fixed,segments:[],scope:'actual two-process native-connected synthetic mixed integration; no packaged/device claim'};
 let preparation,started;
 const segment=async(kind,sourceDir)=>{
  const directory=resolve(out,kind),configPath=resolve(out,`${kind}-config.json`),config={kind,profile,corpusRoot,manifestPath,sourceDir,warm:kind==='resumed'||kind==='initial'&&warm,phasePlan:fixed?.segments[kind]};
  await writeFile(configPath,JSON.stringify(config,null,2));const log=createWriteStream(resolve(out,`${kind}.log`));
  const exit=await new Promise((done,reject)=>{const child=spawn(process.execPath,['scripts/enterprise-run.mjs',directory,engine,`mixed-${kind}`],{stdio:['ignore','pipe','pipe'],env:{...process.env,ENTERPRISE_MIXED_CONFIG:configPath,ENTERPRISE_RESOURCE_DISABLED:kind==='preparation'||disabled?'1':'0'}});child.stdout.pipe(log,{end:false});child.stderr.pipe(log,{end:false});child.once('error',reject);child.once('exit',(code,signal)=>done({code,signal}));});await new Promise(done=>log.end(done));
  const raw=await readFile(resolve(directory,'result.json'),'utf8'),run=JSON.parse(raw);result.segments.push({kind,artifact:directory,resultSha256:digest(run),exit});await writeFile(resolve(out,'result.json'),JSON.stringify(result,null,2));assert.equal(exit.code,0,`failed ${kind} retained`);assert.equal(run.pass,true,run.failure);return run;
 };
 try{
  if(warm){const begin=performance.now();preparation=await segment('preparation');result.preparation={artifact:resolve(out,'preparation'),elapsedMs:performance.now()-begin,providerReceipts:preparation.providerReceipts,parentObservation:preparation.parentObservation,exclusion:'unmeasured fresh initial baseline; never a mutated final-state seed'};}
  started=performance.now();const initial=await segment('initial',warm?resolve(out,'preparation'):undefined);segments.push(initial);const resumed=await segment('resumed',resolve(out,'initial'));segments.push(resumed);
  validateMixedPair(initial,resumed);const manifest=JSON.parse(await readFile(manifestPath,'utf8'));const corpus=assertMixedCorpus(resumed.mixed.corpus,manifest,{appended:true,complete:true});
  result.workload=initial.workload;result.environment=initial.environment;result.nativeBinarySha256=initial.nativeBinarySha256;result.provenance=initial.provenance;result.providerReceipts=initial.providerReceipts+resumed.providerReceipts;
  result.mixedOutcome={initialAuthority:warm?'prepared-initial-corpus-and-unapproved-PR-cache':'genuine-empty-corpus-import-with-pending-generated-bodies',prFacts:resumed.mixed.finalClients.map(client=>client.facts.map(({identity,list,head,requested,draft,queued,terminal,approved})=>({identity,list,head,requested,draft,queued,terminal,approved})).sort((a,b)=>a.identity.localeCompare(b.identity))),ready:resumed.mixed.finalClients.map(client=>[...client.ready].sort()),corpus,actions:resumed.mixed.receipts,pauseSurvivedRestart:true,rollbackPreservedAuthority:true};
  const phasePlan=fixed?Object.fromEntries(['idle','unchanged_refresh','active_ingestion','churn_restart'].map(phase=>[phase,(fixed.segments.initial[phase]??0)+(fixed.segments.resumed[phase]??0)])):null;
  result.resourceIdentity={manifestSha256:manifest.logicalSha256,toolchain:digest(initial.provenance),phasePlan,segmentPhasePlan:fixed?.segments??null,phasePlanStatus:fixed?'fixed':'exploratory-only'};
  if(disabled)result.resources={enabled:false,reason:'runtime_disabled',segments:segments.map(run=>({process:run.process,artifact:result.segments.find(s=>s.kind===run.mixed.kind).artifact}))};
  else{const evidence={schema:2,contract:'mixed',segments:segments.map(run=>({kind:run.mixed.kind,warm:run.warm,process:run.process,evidence:run.resources.evidence}))};result.resources={enabled:true,evidence,summary:summarizeMixedResources(evidence,engine)};validateResourceResult(result);}
  result.parentObservation={wallMs:performance.now()-started,nativeRssPeak:Math.max(...segments.map(run=>run.parentObservation.nativeRssPeak)),commandLatencyMaxMs:Math.max(...segments.map(run=>run.parentObservation.commandLatencyMaxMs)),lost:segments.some(run=>run.parentObservation.lost),artifactBytes:(await bytes(out))-(warm?await bytes(resolve(out,'preparation')):0),meaning:'independent parent measures; wall includes actual restart downtime; preparation artifacts/cost reported separately'};
  result.overlap={requiredForCold:!warm,initial:initial.mixed.overlap,resumed:resumed.mixed.overlap,meaning:'same native clock; completed positive index-work pass excludes cadence sleep; warm overlap only when actually observed'};
  result.restart={actualProcesses:segments.map(run=>run.process),stableProfile:profile,sourceResultSha256:digest(initial),resumedResultSha256:digest(resumed),checkpointSha256:digest(JSON.parse(await readFile(resolve(out,'initial/mixed-checkpoint.json'),'utf8'))),cacheArtifact:resolve(out,'resumed/churn-restart-cache.json'),corpusCacheArtifact:resolve(out,'resumed/mixed-restart-corpus-cache.json')};
  result.pass=true;
 }catch(error){result.failure=error.message;process.exitCode=1;}
 finally{await writeFile(resolve(out,'result.json'),JSON.stringify(result,null,2));console.log(JSON.stringify({mode:'mixed',out,pass:result.pass,failure:result.failure,calibrationEligible:result.calibrationEligible,providerReceipts:result.providerReceipts}));}
 return result;
}
