import {executeMixedPhase} from './mixed-phase.mjs';
import assert from 'node:assert/strict';
import {appendFile,writeFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import {initialOracle,advanceOracle,assertChurnFacts,assertChurnReady,digest} from './churn-plan.mjs';
import {snapshot,navigate,mountContinuity,assertContinuity,observeRoles,observeUntil} from './churn.mjs';
import {appendMixedFixture} from './mixed-fixture.mjs';
import {assertMixedCorpus,assertMixedRollback,corpusAuthority,assertMixedUnreadable} from './mixed-observation.mjs';
const sleep=ms=>new Promise(done=>setTimeout(done,ms));
export async function runMixedSegment({pages,provider,result,out,control,browserResources,config,plan,manifest,state}){
 let oracle=state?.oracle??initialOracle(plan),receipts=structuredClone(state?.receipts??[]),continuity=false;
 const evidence=result.mixed={kind:config.kind,planHash:digest(plan),oracle,receipts,checkpoints:[],phases:[],consumerWitnesses:[],fixtureEffects:[],corpus:null};
 result.samples.push({role:'desktop'},{role:'paired'});
 const save=async()=>{evidence.oracle=oracle;await writeFile(resolve(out,'mixed.json'),JSON.stringify(evidence,null,2));};
 const observe=async()=>{const clients=await Promise.all(pages.map(snapshot));await observeRoles(clients,async(client,i)=>{assert.equal(client.lost,false);assertChurnFacts(oracle,client.facts);assertChurnReady(oracle,client.ready);if(continuity)await assertContinuity(pages[i]);});return clients;};
 const converge=async()=>{const start=performance.now();let last;while(performance.now()-start<180000){try{const clients=await observe();evidence.checkpoints.push({revision:oracle.revision,clients});await save();return clients;}catch(error){last=error;if(/lost|continuity/.test(error.message))throw error;}await sleep(1000);}throw Error(`mixed PR convergence: ${last?.message}`);};
 const waitCorpus=async(check,limit=360000)=>{const start=performance.now();let last;while(performance.now()-start<limit){await observe();const status=await control('corpus-status');try{const value=check(status);evidence.corpus=status;return value??status;}catch(error){last=error;}await sleep(2000);}throw Error(`mixed corpus convergence: ${last?.message}`);};
 const phase=async(name,work)=>{const record={name};evidence.phases.push(record);try{await executeMixedPhase({record,requestedMs:config.phasePlan?.[name],startBoundary:async()=>{if(browserResources)await browserResources.startPhase(name,control);},endBoundary:async()=>{if(browserResources)await browserResources.endPhase(name,control);},work,observe,waitUntil:observeUntil});}finally{await save();}};
 const actions=async(stop)=>{for(const event of plan.events.slice(oracle.revision,stop)){const before=provider.ledger.length,page=pages[event.role==='paired'?1:0];let receipt;if(event.kind==='navigate')await navigate(page,event.next.number);else{if(!(await page.getByText(`Synthetic description ${event.next.number}`,{exact:false}).count()))await navigate(page,event.next.number);await page.getByRole('button',{name:'Approve',exact:true}).first().click();await page.getByRole('button',{name:'Approved',exact:true}).first().waitFor();const writes=provider.ledger.slice(before).filter(row=>row.operation==='review-write'&&row.status===200&&row.terminal==='response-sent'&&row.responseDelivered);assert.equal(writes.length,1,'one real delivered foreground review');receipt=writes[0].mutationReceipt;receipts.push({revision:event.revision,receipt});evidence.fixtureEffects.push({kind:'actual-review',revision:event.revision,receipt:structuredClone(writes[0])});}oracle=advanceOracle(oracle,event,receipt);await converge();}};
 const search=async(query,identity)=>{for(const [i,page] of pages.entries()){const start=await page.evaluate(()=>window.__enterprise.telemetry.length);await page.getByRole('searchbox',{name:'Search transcript content'}).fill(query);await page.getByRole('button',{name:'Search',exact:true}).click();await page.waitForFunction(({start,identity})=>window.__enterprise.telemetry.slice(start).some(row=>row.name==='claude_search_transcripts'&&row.ok&&row.reply?.verdict?.kind==='matches'&&row.reply.verdict.hits.some(hit=>hit.session_id===identity)),{start,identity},{timeout:30000});const witness=await page.evaluate(start=>window.__enterprise.telemetry.slice(start).find(row=>row.name==='claude_search_transcripts'&&row.ok),start);assert.equal(witness.reply.verdict.hits.length,1);assert.equal(witness.reply.verdict.hits[0].session_id,identity);evidence.consumerWitnesses.push({role:i?'paired':'desktop',query,witness,text:(await page.getByRole('region',{name:'Stored synthetic corpus'}).textContent())?.slice(0,12000)});}};
 try{
  if(state){await converge();const cached=await control('corpus-status');assert.deepEqual(corpusAuthority(cached),corpusAuthority(state.corpus),'persisted corpus authority before start');evidence.cachedCorpus=state.cachedCorpusProof;assert.equal(evidence.cachedCorpus.before,0);assert.equal(evidence.cachedCorpus.after,0);}
  await control('start');await converge();
  if(config.kind==='preparation'){
   await control('corpus-start');await waitCorpus(status=>assertMixedCorpus(status,manifest,{complete:true}));await search('mixedneedle001','session-001');evidence.initialBaseline=assertMixedCorpus(evidence.corpus,manifest,{complete:true});
  }else if(config.kind==='initial'){
   await phase('idle',async()=>{});
   await phase('unchanged_refresh',async()=>{await Promise.all(pages.map(page=>page.evaluate(()=>window.__enterprise.refreshQueues())));await converge();await observeUntil({deadline:performance.now()+60000,observe});});
   await phase('active_ingestion',async()=>{
    await appendFile(resolve(config.corpusRoot,'synthetic/unreadable.jsonl'),JSON.stringify({type:'user',timestamp:'2026-10-18T12:01:00Z',message:{role:'user',content:'mixed permission probe append'}})+'\n');
    await control('corpus-unreadable-on');await actions(1);await control('corpus-start');await actions(plan.checkpointRevision);
    await waitCorpus(status=>{assertMixedCorpus(status,manifest);assertMixedUnreadable(status);});
    await search('mixedneedle001','session-001');
    for(const page of pages)await mountContinuity(page);continuity=true;
    const before=await control('corpus-status');await writeFile(resolve(out,'before-link-fault.json'),JSON.stringify(before,null,2));await control('corpus-fault-on');const append=await appendMixedFixture({profile:config.profile,root:config.corpusRoot},manifest);evidence.fixtureEffects.push({kind:'declared-append',...append});
    await waitCorpus(status=>{assertMixedRollback(before,status);assertMixedCorpus(status,manifest,{appended:true,linksAppended:false});});
    evidence.rollback={before,after:evidence.corpus};await control('corpus-pause');evidence.corpus=await control('corpus-status');assert.equal(evidence.corpus.stored.paused,true);
   });
   await phase('churn_restart',async()=>{const before=await control('corpus-status');await observeUntil({deadline:performance.now()+5000,observe});const after=await control('corpus-status');assert.deepEqual(corpusAuthority(after),corpusAuthority(before),'paused accepted corpus authority retained');evidence.corpus=after;});
  }else{
   assert.equal(config.kind,'resumed');assert.ok(state);
   await phase('churn_restart',async()=>{
    await control('corpus-start');const before=await control('corpus-status');await waitCorpus(status=>{assert.equal(status.lastPass?.indexOutcome,'Paused');assert.deepEqual(corpusAuthority(status),corpusAuthority(before));},30000);evidence.pauseAfterRestart=true;
    await navigate(pages[1],52);await control('corpus-resume');await actions(plan.events.length);
    await waitCorpus(status=>assertMixedCorpus(status,manifest,{appended:true,complete:true}));await search('mixedappendneedle','session-001');
    evidence.corpus=await control('corpus-status');evidence.finalCorpus=assertMixedCorpus(evidence.corpus,manifest,{appended:true,complete:true});assert.equal(evidence.corpus.stored.paused,false);
   });
  }
  const clients=await observe();evidence.finalClients=clients;assert.equal(oracle.revision,config.kind==='preparation'?0:config.kind==='initial'?plan.checkpointRevision:plan.events.length);
  const writes=provider.ledger.filter(row=>row.operation==='review-write');assert.equal(writes.length,config.kind==='preparation'?0:1,'exact measured foreground provider writes');assert.ok(writes.every(row=>row.status===200&&row.responseDelivered));evidence.pass=true;
 }finally{await save();}
}
