import assert from 'node:assert/strict';
import {readFile,writeFile,readdir} from 'node:fs/promises';
import {resolve,dirname} from 'node:path';
import {createHash} from 'node:crypto';
import {assertConverged,convergencePolicy,releaseWithPublication,waitForFreshTraversal} from './convergence.mjs';
const fields=['number','id','updatedAt','headRefOid','state','isDraft','isInMergeQueue','mergeQueueEntry','reviewDecision','closedAt','mergedAt'];
export const baselineRows=data=>data.rows.filter(row=>row.number<=286).map(row=>Object.fromEntries(fields.map(field=>[field,row[field]])));
export function restoreBaseline(data,baseline){
 assert.equal(baseline.length,286);assert.equal(new Set(baseline.map(row=>row.number)).size,286);
 for(const saved of baseline){const row=data.rows.find(row=>row.number===saved.number);assert.ok(row);assert.equal(row.id,saved.id);Object.assign(row,saved);}
}
export async function loadConvergenceRestart(inheritedProfile,copiedProfile,provider){
 assert.ok(inheritedProfile,'convergence-restart requires the original marked profile');
 await readFile(resolve(inheritedProfile,'.enterprise-synthetic'));
 const dir=dirname(resolve(inheritedProfile)),inputSha256={},files={};
 for(const name of ['convergence.json','convergence-baseline.json','result.json']){const bytes=await readFile(resolve(dir,name));files[name]=JSON.parse(bytes);inputSha256[name]=createHash('sha256').update(bytes).digest('hex');}
 assert.equal(files['convergence.json'].pass,true);assert.equal(files['result.json'].pass,true);assert.equal(files['result.json'].shutdown.clean,true);assert.equal(files['result.json'].exit.code,0);
 const profileSha256={};async function compare(relative=''){for(const entry of await readdir(resolve(inheritedProfile,relative),{withFileTypes:true})){const name=relative?`${relative}/${entry.name}`:entry.name;if(entry.isDirectory())await compare(name);else{const a=await readFile(resolve(inheritedProfile,name)),b=await readFile(resolve(copiedProfile,name));assert.ok(a.equals(b),'copied profile must be byte identical before startup');profileSha256[name]=createHash('sha256').update(a).digest('hex');}}}await compare();
 restoreBaseline(provider.data,files['convergence-baseline.json']);
 // Mounting the production hook can request a live page. Hold its actual stale
 // payload until both clients have rendered the independently expected cache.
 provider.fault.holdSearch={list:'reviewing',after:null,remaining:1};
 return {expected:files['convergence.json'].expected,inputSha256,profileSha256,inheritedProfile:resolve(inheritedProfile),policy:convergencePolicy};
}
export async function captureRestartCache({nativeCall,provider,state,out}){
 const evidence={...state,cache:[],providerBefore:provider.ledger.length};
 for(const role of ['desktop','paired'])for(const list of ['authored','reviewing']){
  const response=await nativeCall(role,'get_source_snapshot',{source:{provider:'github',host:'github.com'},list});assert.equal(response.status,200);const body=await response.json(),snapshot=body[role==='desktop'?'value':'wire'];
  assert.ok(['credential_bound','live_verified'].includes(snapshot.ownership.state),'persisted owner binding must qualify cache without list refresh');
  const rows=snapshot.data.prs;assert.deepEqual(rows.map(row=>`${row.repo}/${row.number}`).sort(),state.expected[list]);
  evidence.cache.push({role,list,reply:body});
 }
 assert.equal(provider.ledger.length,evidence.providerBefore,'cache commands perform no provider IO');evidence.providerAfter=provider.ledger.length;
 await writeFile(resolve(out,'convergence-restart-cache.json'),JSON.stringify(evidence,null,2));return evidence;
}
export async function runConvergenceRestart({pages,provider,result,out,profile,state,control}){
 const evidence={policy:convergencePolicy,expected:state.expected,checkpoints:[],cacheEvidence:'convergence-restart-cache.json'};
 const capture=async name=>{const clients=await Promise.all(pages.map(page=>page.evaluate(()=>({inventory:window.__enterprise.inventory(),ready:window.__enterprise.readyEligibility(),source:window.__enterprise.sourceEvidence()}))));for(const client of clients)assertConverged(client,state.expected);evidence.checkpoints.push({name,clients,providerCount:provider.ledger.length});};
 const delay=ms=>new Promise(r=>setTimeout(r,ms));
 try{
  const started=performance.now();let matched=false;while(performance.now()-started<convergencePolicy.localPropagationMs){try{await capture('mounted-persisted-cache');matched=true;break;}catch{await delay(100);}}assert.ok(matched,'both real clients render converged retained cache');
  await control('start');const holdStart=performance.now();while(!provider.heldCount&&performance.now()-holdStart<convergencePolicy.inventoryMs)await delay(100);
  const held=provider.ledger.filter(entry=>entry.held&&!entry.released);assert.ok(held.length,'restart must materialize an actual stale provider page');
  await capture('stale-page-held');evidence.stalePublication=await releaseWithPublication({pages,provider,profile,expected:state.expected});
  evidence.freshTraversal=await waitForFreshTraversal({pages,provider,consumption:evidence.stalePublication,expected:state.expected});
  evidence.pass=true;result.samples.push({role:'desktop'},{role:'paired'});result.convergenceRestart={pass:true};
 }finally{provider.release();await writeFile(resolve(out,'convergence-restart.json'),JSON.stringify(evidence,null,2));}
}
