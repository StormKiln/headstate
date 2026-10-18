import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {readFile,writeFile,readdir} from 'node:fs/promises';import {resolve,dirname} from 'node:path';import {createHash} from 'node:crypto';
import {digest,createChurnPlan,initialOracle,advanceOracle} from './churn-plan.mjs';import {assertCompatibleWorkload} from './workload.mjs';
export async function profileHashes(path){const result={};async function walk(relative=''){for(const entry of (await readdir(resolve(path,relative),{withFileTypes:true})).sort((a,b)=>a.name.localeCompare(b.name))){const name=relative?`${relative}/${entry.name}`:entry.name;if(entry.isDirectory())await walk(name);else if(entry.isFile())result[name]=createHash('sha256').update(await readFile(resolve(path,name))).digest('hex');else assert.fail('profile cannot contain symlinks');}}await walk();return result;}
export function validateCheckpoint(record,source,workload){
 assert.equal(record.schema,1);assert.equal(record.segmentPass,true);assert.equal(source.pass,true,'source must pass');assert.equal(source.shutdown.clean,true,'source clean shutdown');assert.equal(source.exit.code,0);
 assert.deepEqual(record.activeSegmentsMs,source.churn?.activeSegmentsMs,'source actual active duration evidence must match');
 assert.ok(Array.isArray(record.activeSegmentsMs)&&record.activeSegmentsMs.length>0,'recorded active segment durations required');for(const value of record.activeSegmentsMs)assert.ok(Number.isFinite(value)&&value>=0);assert.ok(record.activeSegmentsMs.at(-1)<=source.parentObservation.wallMs,'active segment cannot exceed measured process wall time');
 assert.equal(source.churn.segmentPass,true,'source segment obligations must pass');assert.equal(source.churn.endRevision,record.oracle.revision,'checkpoint must match actually executed source revision');assert.equal(source.churn.planHash,digest(record.plan),'source plan hash');assert.deepEqual(source.churn.events.map(event=>event.revision),record.plan.events.slice(source.churn.startRevision,record.oracle.revision).map(event=>event.revision),'source execution obligations cannot be skipped or replayed');
 assertCompatibleWorkload(record.plan.workload,workload);
 assert.deepEqual(record.plan,createChurnPlan(workload),'plan must regenerate from declared seed, never provider rows');
 for(const [field,key] of [['plan','plan'],['oracle','oracle'],['provider','provider'],['profileHashes','profile']])assert.equal(digest(record[field]),record.hashes[key],`${field} hash`);
 let expected=initialOracle(record.plan);for(const event of record.plan.events.slice(0,record.oracle.revision))expected=advanceOracle(expected,event,record.receipts.find(r=>r.revision===event.revision)?.receipt);
 assert.deepEqual(record.oracle,expected,'independent oracle replay must match checkpoint');assertProviderMatchesOracle(record.provider,expected);return record;
}
export async function saveCheckpoint({out,profile,provider,plan,oracle,receipts,activeSegmentsMs,result}){
 if(!result.pass||!result.shutdown?.clean||result.exit?.code!==0)return;
 const savedProvider=JSON.parse(JSON.stringify(provider.data,(key,value)=>key==='pullRequest'?undefined:value));const hashes=await profileHashes(profile);
 const record={schema:1,segmentPass:true,planComplete:oracle.revision===plan.events.length,plan,oracle,provider:savedProvider,profileHashes:hashes,receipts,activeSegmentsMs,hashes:{plan:digest(plan),oracle:digest(oracle),provider:digest(savedProvider),profile:digest(hashes)}};
 await writeFile(resolve(out,'churn-checkpoint.json'),JSON.stringify(record,null,2));
}
export async function loadChurnRestart({inheritedProfile,profile,provider,workload,expectedNativeBinaryHash}){
 assert.ok(inheritedProfile,'churn-restart requires successful marked source profile');await readFile(resolve(inheritedProfile,'.enterprise-synthetic'));
 const record=JSON.parse(await readFile(resolve(dirname(inheritedProfile),'churn-checkpoint.json'),'utf8'));const source=JSON.parse(await readFile(resolve(dirname(inheritedProfile),'result.json'),'utf8'));validateCheckpoint(record,source,workload);
 const metadataBytes=await readFile(resolve(dirname(inheritedProfile),'metadata.json'));const metadata=JSON.parse(metadataBytes);verifyChurnProvenance(metadata,source,expectedNativeBinaryHash);const sourceProvenance={commit:metadata.commit,sourceTree:metadata.sourceTree,harnessCode:metadata.harnessCode,browserAssets:metadata.browserAssets,metadataSha256:createHash('sha256').update(metadataBytes).digest('hex'),resultSha256:createHash('sha256').update(await readFile(resolve(dirname(inheritedProfile),'result.json'))).digest('hex'),checkpointSha256:createHash('sha256').update(await readFile(resolve(dirname(inheritedProfile),'churn-checkpoint.json'))).digest('hex')};
 assert.deepEqual(await profileHashes(inheritedProfile),record.profileHashes,'source profile hash');assert.deepEqual(await profileHashes(profile),record.profileHashes,'byte-identical copied profile');
 assert.ok(record.oracle.revision<record.plan.events.length,'source has unexecuted obligations');
 const data=structuredClone(record.provider);for(const row of data.rows){row.repository=data.repos.find(repo=>repo.nameWithOwner===row.repository.nameWithOwner);row.headRepository=row.repository;}Object.assign(provider.data,data);return {...record,sourceProvenance,sourceReadyCoverage:source.churn.readyCoverage??'absent-facts-only'};
}
export async function captureChurnCache({nativeCall,provider,state,out}){
 const before=provider.ledger.length,observations=[],evidence={before,after:null,observations,pass:false};
 try{
 for(const role of ['desktop','paired'])for(const list of ['authored','reviewing']){const response=await nativeCall(role,'get_source_snapshot',{source:{provider:'github',host:'github.com'},list});assert.equal(response.status,200);const body=await response.json(),snapshot=body[role==='desktop'?'value':'wire'];observations.push({role,list,snapshot});assert.ok(['credential_bound','live_verified'].includes(snapshot.ownership.state));const expected=state.oracle.rows.filter(row=>row.terminal==='open'&&(list==='authored'?row.authored:row.requested));assert.deepEqual(snapshot.data.prs.map(r=>`${r.repo}/${r.number}`).sort(),expected.map(r=>r.identity).sort());for(const row of snapshot.data.prs){const wanted=expected.find(r=>r.identity===`${row.repo}/${row.number}`);assert.equal(row.head_oid,wanted.head);assert.equal(row.is_draft,wanted.draft);assert.equal(row.in_merge_queue,wanted.queued);assert.equal(row.requested_reviewers.includes('synthetic-viewer'),wanted.requested,'cached requested membership');assert.equal(row.latest_reviews.some(review=>review.author==='synthetic-viewer'&&review.state==='APPROVED'&&review.commit_oid===row.head_oid),wanted.approved,'cached current-head approval');}}
 assert.equal(provider.ledger.length,before,'cached authority checked before provider polling');evidence.pass=true;
 }finally{evidence.after=provider.ledger.length;await writeFile(resolve(out,'churn-restart-cache.json'),JSON.stringify(evidence,null,2));}
}

export function assertProviderMatchesOracle(data,oracle){
 const open=data.rows.filter(row=>row.state==='OPEN');assert.deepEqual(open.map(row=>`${row.repository.nameWithOwner}/${row.number}`).sort(),oracle.rows.filter(row=>row.terminal==='open').map(row=>row.identity).sort(),'restored provider exact open identities');
 for(const expected of oracle.rows){const row=data.rows.find(row=>`${row.repository.nameWithOwner}/${row.number}`===expected.identity);assert.ok(row,'restored planned identity exists');assert.equal(row.headRefOid,expected.head);assert.equal(row.commits.nodes[0].commit.oid,expected.head);assert.equal(row.state.toLowerCase(),expected.terminal);assert.equal(row.isDraft,expected.draft);assert.equal(row.isInMergeQueue,expected.queued);assert.equal(row.reviewRequests.nodes.some(r=>r.requestedReviewer.login==='synthetic-viewer'),expected.requested);assert.equal(row.latestReviews.nodes.some(r=>r.author.login==='synthetic-viewer'&&r.state==='APPROVED'&&r.commit.oid===expected.head),expected.approved);}
}

export function verifyChurnProvenance(metadata,source,expectedNativeBinaryHash){
 assert.equal(metadata.dirty,false,'restart source must identify committed harness code');assert.match(metadata.commit,/^[0-9a-f]{40}$/);assert.equal(source.environment.sourceRevision,metadata.commit);assert.equal(source.nativeBinarySha256,metadata.nativeBinarySha256);if(expectedNativeBinaryHash)assert.equal(source.nativeBinarySha256,expectedNativeBinaryHash,'restart native binary must match source');
 assert.equal(execFileSync('git',['rev-parse',`${metadata.commit}^{tree}`],{encoding:'utf8'}).trim(),metadata.sourceTree,'source tree provenance');
 const required=['scripts/enterprise-run.mjs','scripts/enterprise/provider.mjs','scripts/enterprise/churn-plan.mjs','scripts/enterprise/churn.mjs','scripts/enterprise/churn-restart.mjs','scripts/enterprise/workload.mjs','scripts/enterprise/stats-retirement.mjs','src/harness/enterprise.tsx','src/harness/enterpriseTransport.ts','src/harness/enterpriseFacts.ts'];assert.deepEqual(metadata.harnessCode.map(file=>file.path).sort(),required.sort(),'complete source file provenance');
 for(const file of metadata.harnessCode)assert.equal(createHash('sha256').update(execFileSync('git',['show',`${metadata.commit}:${file.path}`])).digest('hex'),file.sha256,`committed source bytes ${file.path}`);
}
