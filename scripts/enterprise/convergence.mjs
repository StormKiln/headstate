import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import {joinHeldReviewing} from './convergence-join.mjs';
import {assertConsumption,assertRecovery,consumptionLineage} from './convergence-consumption.mjs';

export const convergencePolicy=Object.freeze({inventoryMs:180000,localPropagationMs:10000,failureCycleMs:180000,meaning:'Synthetic diagnostic ceilings, not a field SLA; optional advisory freshness is excluded'});
const key=number=>`synthetic-lab/repo-${(number-1)%50+1}/${number}`;
export function initialConvergenceExpected(){return {authored:Array.from({length:50},(_,i)=>key(i+1)).sort(),reviewing:Array.from({length:236},(_,i)=>key(i+51)).sort(),ready:Array.from({length:130},(_,i)=>key(i+51)).sort(),changes:[]};}
export function changeExpected(expected,number,{member,ready},reason){
 assert.ok(number>=51&&number<=286);assert.ok(!ready||member);
 for(const [field,include] of [['reviewing',member],['ready',ready]]){const next=new Set(expected[field]);if(include)next.add(key(number));else next.delete(key(number));expected[field]=[...next].sort();}
 expected.changes.push({number,member,ready,reason});return expected;
}
export function assertConverged(observed,expected){assert.deepEqual(observed.inventory.authored,expected.authored);assert.deepEqual(observed.inventory.reviewing,expected.reviewing);assert.deepEqual(observed.ready,expected.ready);}
const delay=ms=>new Promise(r=>setTimeout(r,ms));
const snapshot=page=>page.evaluate(()=>({at:performance.now(),wallTime:Date.now(),inventory:window.__enterprise.inventory(),ready:window.__enterprise.readyEligibility(),source:window.__enterprise.sourceEvidence(),footer:document.querySelector('[data-testid="inventory-footer"]')?.textContent,commands:window.__enterprise.telemetry.filter(e=>e.kind==='call'&&['get_pr_detail','act_on_pr'].includes(e.name))}));

export async function releaseWithPublication({pages,provider,profile,expected,requestId,joined,ceilingMs=convergencePolicy.localPropagationMs}){
 const held=provider.ledger.filter(entry=>entry.held&&!entry.released);assert.equal(held.length,1);
 const releasedAt=Date.now(),releasedMono=performance.now();provider.release();
 let lastError;
 while(performance.now()-releasedMono<=ceilingMs){
  try{
   const events=(await readFile(resolve(profile,'native.ndjson'),'utf8')).split('\n').filter(Boolean).map(JSON.parse);
   const calls=await pages[0].evaluate(()=>window.__enterprise.telemetry.filter(e=>e.kind==='call'&&e.args?.requestId&&e.reply?.update?.list==='reviewing'));
   for(const completion of calls){
    if(requestId&&completion.args.requestId!==requestId)continue;
    let lineage;try{lineage=consumptionLineage(events,completion.callId,held[0].id);}catch{continue;}
    if(joined){assert.equal(lineage.commandId,joined.commandId);assert.equal(lineage.slotId,joined.slotId);}
    const clients=await Promise.all(pages.map(snapshot));
    const proof=assertConsumption({requestId:completion.args.requestId,commandId:completion.callId,held:held[0],lineage,completion,expected,clients,releasedAt,elapsedMs:performance.now()-releasedMono,ceilingMs});
    return {releasedAt,releasedMono,elapsedMs:performance.now()-releasedMono,held,clients,...proof};
   }
  }catch(error){lastError=error;}
  await delay(50);
 }
 throw Error(`Exact held native completion not proved within local ceiling: ${lastError?.message??'completion withheld'}`);
}
export async function waitForFreshTraversal({pages,provider,profile,consumption,expected}){
 while(performance.now()-consumption.releasedMono<=convergencePolicy.inventoryMs){
  const clients=await Promise.all(pages.map(async page=>({...await snapshot(page),publications:await page.evaluate(()=>window.__enterprise.telemetry.filter(e=>e.kind==='event'&&e.name==='source-poll-status').map(e=>e.reply))})));
  const events=(await readFile(resolve(profile,'native.ndjson'),'utf8')).split('\n').filter(Boolean).map(JSON.parse);
  const providerReads=provider.ledger.filter(entry=>entry.at>consumption.releasedMono&&entry.searches?.some(search=>search.query.includes('review-requested:@me')));
  try{
   for(const client of clients)assertConverged(client,expected);
   const proof=assertRecovery({releasedAt:consumption.releasedAt,releasedMono:consumption.releasedMono,elapsedMs:performance.now()-consumption.releasedMono,ceilingMs:convergencePolicy.inventoryMs,provider:providerReads,clients,events,consumed:consumption.completion.reply.update});
   return {elapsedMs:performance.now()-consumption.releasedMono,provider:providerReads,clients,proof};
  }catch{}
  await delay(100);
 }
 throw Error('No subsequent genuine complete reviewing traversal within original180s recovery ceiling');
}

export function assertPrimaryDetailCount(ledger,number,count){
 const acquisitions=ledger.filter(entry=>entry.primaryDetails?.some(identity=>identity.number===number&&identity.repo===key(number).slice(0,key(number).lastIndexOf('/'))));
 assert.equal(acquisitions.length,count,`primary detail acquisitions for PR ${number}; checks/thread continuations and inventory cadence excluded by parsed primary fields`);return acquisitions;
}

/** Actual provider → native publication → independent mounted clients. */
export async function runConvergence({pages,provider,result,out,profile,control}){
 const expected=initialConvergenceExpected();
 const evidence={policy:convergencePolicy,expected:structuredClone(expected),phases:[],checkpoints:[],exclusions:['Updater/settings controls: mounted fragment is the same production GitHub inventory footer','Restart requires convergence-restart with this successful run profile; account replacement and newer-head/reopen authority require complementary native full-path tests']};
 const save=()=>writeFile(resolve(out,'convergence.json'),JSON.stringify(evidence,null,2));
 const capture=async name=>{const value={name,at:performance.now(),providerCount:provider.ledger.length,clients:await Promise.all(pages.map(snapshot))};evidence.checkpoints.push(value);await save();return value;};
 const waitUntil=async(test,ceilingMs,message)=>{const start=performance.now();while(performance.now()-start<ceilingMs){if(await test())return;await delay(100);}throw Error(message);};
 const population=async(ceilingMs=convergencePolicy.localPropagationMs)=>waitUntil(async()=>{const states=await Promise.all(pages.map(snapshot));try{for(const state of states)assertConverged(state,expected);return true;}catch{return false;}},ceilingMs,'actual mounted inventory/Ready did not match independent expected identities');
 const open=async(number,page=pages[0])=>{const back=page.getByRole('button',{name:'Back to list',exact:true});if(await back.count())await back.click();await page.getByRole('button',{name:new RegExp(`Synthetic review ${number}(?:\\D|$)`)}).first().click();await page.getByText(`Synthetic description ${number}`,{exact:false}).waitFor();};
 const providerRow=number=>{const row=provider.data.rows.find(row=>row.number===number);assert.ok(row);return row;};
 const external=(number,patch)=>{const row=providerRow(number);Object.assign(row,patch,{updatedAt:new Date(Math.max(Date.now(),Date.parse(row.updatedAt)+1)).toISOString()});};
 const freshRead=async(number,patch,expectation,reason)=>{
  const before=provider.ledger.length,clickAt=performance.now();external(number,patch);changeExpected(expected,number,expectation,reason);
  const offset=await pages[0].evaluate(()=>window.__enterprise.telemetry.length);
  await open(number);
  await pages[0].waitForFunction(({number,offset})=>window.__enterprise.telemetry.slice(offset).some(e=>e.name==='get_pr_detail'&&e.ok&&e.args?.number===number),{number,offset},{timeout:30000});
  const receipt=await pages[0].evaluate(({number,offset})=>window.__enterprise.telemetry.slice(offset).find(e=>e.name==='get_pr_detail'&&e.ok&&e.args?.number===number),{number,offset});
  // The clock anchor is successful receipt completion, not the click or request start.
  await population(Math.max(1,convergencePolicy.localPropagationMs-(await pages[0].evaluate(at=>performance.now()-at,receipt.at+receipt.duration))));
  await delay(2000); // Settled observation interval, within the unchanged ten-second local ceiling.
  assert.ok(await pages[0].evaluate(at=>performance.now()-at,receipt.at+receipt.duration)<=convergencePolicy.localPropagationMs,'propagation and post-publication settlement remain within declared local ceiling');
  const primaryAcquisitions=assertPrimaryDetailCount(provider.ledger.slice(before),number,1);
  const redundant=await pages[0].evaluate(offset=>window.__enterprise.telemetry.slice(offset).filter(e=>e.kind==='call'&&['refresh_source','refresh_now','get_reviewing'].includes(e.name)),offset);
  assert.deepEqual(redundant,[],'propagating accepted detail must not issue a new inventory request');
  evidence.phases.push({name:reason,number,clickAt,receipt,settlementMs:2000,primaryAcquisitions,propagationMs:await pages[0].evaluate(at=>performance.now()-at,receipt.at+receipt.duration),provider:provider.ledger.slice(before)});
  await capture(reason);
 };
 let heldJoin;
 const releaseOldSearch=async()=>{const consumption=await releaseWithPublication({pages,provider,profile,expected,requestId:heldJoin.requestId,joined:heldJoin});evidence.phases.push({name:'held-page-processed',...consumption});await save();evidence.phases.push({name:'subsequent-fresh-traversal',...await waitForFreshTraversal({pages,provider,profile,consumption,expected})});};
 const holdOldSearch=async()=>{
  provider.fault.holdSearch={list:'reviewing',after:null,remaining:1};
  heldJoin=await joinHeldReviewing({page:pages[0],provider,profile,inventoryMs:convergencePolicy.inventoryMs,joinMs:convergencePolicy.localPropagationMs});
  evidence.phases.push({name:'manual-refresh-joined-held-scan',...heldJoin});await save();
 };
 const action=async(number,label,expectation,reason)=>{
  await open(number);const offset=await pages[0].evaluate(()=>window.__enterprise.telemetry.length);const before=provider.ledger.length;
  await pages[0].getByRole('button',{name:label,exact:true}).first().click({timeout:convergencePolicy.inventoryMs});
  await pages[0].waitForFunction(({number,offset})=>window.__enterprise.telemetry.slice(offset).some(e=>e.name==='act_on_pr'&&e.ok&&e.args?.number===number),{number,offset},{timeout:30000});
  const receipt=await pages[0].evaluate(({number,offset})=>window.__enterprise.telemetry.slice(offset).find(e=>e.name==='act_on_pr'&&e.ok&&e.args?.number===number),{number,offset});
  changeExpected(expected,number,expectation,reason);await population(Math.max(1,convergencePolicy.localPropagationMs-(await pages[0].evaluate(at=>performance.now()-at,receipt.at+receipt.duration))));
  evidence.phases.push({name:reason,number,receipt,provider:provider.ledger.slice(before)});await capture(reason);
 };
 await save(); // The independent population and approved bounds predate scheduler start.
 try{
  await control('start');await population(convergencePolicy.inventoryMs);await capture('cold236reviewing130ready');
  for(const [i,page] of pages.entries())await page.screenshot({path:resolve(out,`convergence-cold-${i}.png`),fullPage:true});
  result.samples.push({role:'desktop'},{role:'paired'});
  await holdOldSearch();
  await freshRead(51,{isInMergeQueue:true,mergeQueueEntry:{state:'QUEUED'}},{member:true,ready:false},'fresh-detail-queued');
  await freshRead(52,{reviewDecision:'APPROVED'},{member:true,ready:false},'fresh-detail-aggregate-approved');
  await freshRead(53,{state:'MERGED',closedAt:new Date().toISOString(),mergedAt:new Date().toISOString()},{member:false,ready:false},'fresh-detail-merged');
  await freshRead(54,{state:'CLOSED',closedAt:new Date().toISOString()},{member:false,ready:false},'fresh-detail-closed');
  await releaseOldSearch();await population();await capture('old-search-after-detail-did-not-resurrect');
  assert.equal(expected.reviewing.length,234);assert.equal(expected.ready.length,126);
  await holdOldSearch();
  await action(55,'Add to merge queue',{member:true,ready:false},'acknowledged-enqueue');
  await action(56,'Convert to draft',{member:true,ready:false},'acknowledged-draft');
  await releaseOldSearch();await population();await capture('old-search-after-actions-did-not-requalify');
  await action(55,'Remove from merge queue',{member:true,ready:true},'acknowledged-dequeue');
  await action(56,'Mark ready for review',{member:true,ready:true},'acknowledged-ready');
  const reuseStart=provider.ledger.length;await open(60);await delay(2000);assertPrimaryDetailCount(provider.ledger.slice(reuseStart),60,1);
  await open(60);await delay(2000);assertPrimaryDetailCount(provider.ledger.slice(reuseStart),60,1);
  evidence.phases.push({name:'ordinary-back-open-cache-reuse',number:60,settlementMs:2000,provider:provider.ledger.slice(reuseStart)});
  await pages[0].getByText('Open report',{exact:true}).click();
  const draft=pages[0].getByPlaceholder('Reply…').first();await draft.fill('Retained convergence backward draft');
  await draft.evaluate(node=>{node.focus();node.setSelectionRange(3,17,'backward');window.__convergenceDraft=node;});
  for(let cycle=0;cycle<2;cycle++){
   const before=await capture(`before-finite-failure-${cycle}`),ledgerStart=provider.ledger.length;
   provider.fault.failSearch=cycle===0?{list:'reviewing',after:'cursor-50',remaining:3}:{list:'reviewing',after:null,remaining:1,waitForTail:true};
   await control('wake');
   await waitUntil(()=>provider.ledger.slice(ledgerStart).some(entry=>entry.targetedFailure),convergencePolicy.failureCycleMs,'finite failure was not exercised');
   await waitUntil(async()=>{const current=await snapshot(pages[0]);return ['failed','retrying'].includes(current.source?.phase);},convergencePolicy.localPropagationMs,'finite failure was not published to active reviewing list');
   const failed=await capture(`finite-failure-${cycle}`);
   const returned=new Set(provider.ledger.slice(ledgerStart).filter(entry=>entry.status===200).flatMap(entry=>(entry.searches??[]).flatMap(search=>search.ids)));
   let unchanged=0;
   for(const old of before.clients[0].source.prs){if(returned.has(old.id))continue;const actual=failed.clients[0].source.prs.find(row=>row.id===old.id);assert.deepEqual(actual?.observation,old.observation,'untouched row observation must not be reset by a finite failure');unchanged++;}
   assert.ok(unchanged>0,'failure proof needs identities not returned by intervening pages');
   assert.match(failed.clients[0].footer,/Could not|Retrying/);
   assertConverged(failed.clients[0],expected);assertConverged(failed.clients[1],expected);
   const continuity=await pages[0].evaluate(()=>{const n=window.__convergenceDraft;return {connected:n?.isConnected,focused:document.activeElement===n,value:n?.value,start:n?.selectionStart,end:n?.selectionEnd,direction:n?.selectionDirection,disclosure:[...document.querySelectorAll('details')].some(d=>d.open&&d.querySelector('summary')?.textContent==='Open report')};});
   assert.deepEqual(continuity,{connected:true,focused:true,value:'Retained convergence backward draft',start:3,end:17,direction:'backward',disclosure:true});
   provider.fault.failSearch=undefined;
   await waitUntil(async()=>{const current=await snapshot(pages[0]);return current.source?.phase==='ready'&&current.source?.coverage==='complete';},convergencePolicy.failureCycleMs,'finite failed pass did not recover');
   await population();evidence.phases.push({name:`failure-recovery-${cycle}`,unchanged,continuity,provider:provider.ledger.slice(ledgerStart)});await capture(`recovered-${cycle}`);
  }
  evidence.expected=structuredClone(expected);evidence.pass=true;result.convergence={pass:true,reviewing:expected.reviewing.length,ready:expected.ready.length};
 }finally{provider.release();evidence.expected=structuredClone(expected);await save();}
}
