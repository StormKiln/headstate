import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import {documentCost,reconcileQuota} from './quota.mjs';
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms));
export function assertQuotaScenario(e,ledger,profile){
 assert.ok(profile.graphql.windowSeconds>120&&profile.rest.capacity>500);
 assert.ok(ledger.length&&ledger.every(r=>r.fault==='healthy'),'natural scenario never injects limits');
 for(const r of ledger)assert.equal(r.quota.cost,documentCost(profile,r.quotaShape),'cost matches request AST/REST shape');
 const accounting=reconcileQuota(profile,ledger.map(r=>r.quota));
 const reserve=ledger.find(r=>r.id===e.reserve?.receiptId);
 assert.ok(reserve?.bucket==='rest'&&reserve.quota.before>500&&reserve.quota.after<=500&&reserve.quota.after>0);
 assert.ok(e.reserve.admission.rest.reserveCooldownMs>0,'actual production reserve observed');
 assert.ok(e.foreground?.admission?.rest.reserveCooldownMs>0,'foreground action occurs during real REST reserve');
 assert.ok(e.foreground?.status===200&&ledger.some(r=>r.id===e.foreground.receiptId&&r.operation==='review-write'&&r.status===200&&r.quota.after>0),'real foreground review during REST reserve');
 const exhaustion=ledger.find(r=>r.id===e.exhaustion?.receiptId);
 assert.ok(exhaustion?.quota.naturalLimit&&exhaustion.status===403&&exhaustion.quota.charged===0,'naturally denied transport required');
 assert.ok(e.exhaustion.admission.graphql.primaryCooldownMs>0,'native primary cooldown observed');
 assert.equal(e.blockedPublications?.length,2);
 for(const blocked of e.blockedPublications)assert.ok(['failed','not_asked','retrying','partial','unknown'].includes(blocked.phase),'actual incomplete or denied refresh');
 assert.ok(e.quiescence?.durationMs>=10000&&e.quiescence.restRequests===0&&e.quiescence.graphqlRequests<=2,'bounded blocked-window traffic');
 const allBlocked=ledger.filter(r=>r.quota.receivedAt>=exhaustion.quota.receivedAt+2000&&r.quota.receivedAt<exhaustion.quota.reset*1000);assert.equal(allBlocked.filter(r=>r.bucket==='rest').length,0,'REST quiescence across full blocked window');assert.ok(allBlocked.filter(r=>r.bucket==='graphql').length<=2,'bounded GraphQL traffic across full blocked window');
 assert.equal(e.retained?.length,2);
 for(const r of e.retained){assert.ok(r.before.authored.length+r.before.reviewing.length>0);for(const kind of ['authored','reviewing'])assert.ok(r.before[kind].every(id=>r.after[kind].includes(id)),'accepted partial inventory retained');}
 for(const [i,blocked] of e.blockedPublications.entries()){assert.deepEqual(blocked.prs.map(r=>`${r.repo}/${r.number}`).sort(),e.retained[i].before.reviewing,'blocked publication retains exact accepted identities');for(const r of blocked.prs){assert.equal(r.head_oid,`head-${r.number}`,'retained head fact');assert.equal(r.base_ref,'main');assert.equal(r.is_draft,false);}}
 const recovery=ledger.find(r=>r.id===e.recovery?.receiptId);
 assert.ok(recovery?.status===200&&recovery.bucket==='graphql'&&recovery.quota.windowId!==exhaustion.quota.windowId&&recovery.quota.receivedAt>=exhaustion.quota.reset*1000,'real reset receipt');
 assert.ok(e.recovery.autonomous&&e.recovery.native.graphql.remaining>0&&e.recovery.inventories.every(i=>i.authored.length===50&&i.reviewing.length===150),'native and mounted recovery');
 assert.equal(e.recovery.publications?.length,2);assert.equal(e.recovery.sources?.length,2);
 for(const [i,p] of e.recovery.publications.entries()){const blocked=e.blockedPublications[i],source=e.recovery.sources[i];assert.ok(e.recovery.accepted.some(a=>a.operation==='scan-slot'&&a.stage==='accepted-reviewing'&&a.code===p.receipt_revision),'actual native accepted revision required');assert.ok(p.session===blocked.session&&p.receipt_revision>blocked.receipt_revision&&p.phase==='ready'&&p.coverage==='complete'&&Date.parse(p.last_received_at)>=exhaustion.quota.reset*1000,'new accepted source publication after reset');assert.ok(source.phase==='ready'&&source.coverage==='complete'&&source.lastReceivedAt===p.last_received_at,'mounted source consumes recovered publication');}
 return accounting;
}
export async function runQuotaScenario({pages,provider,result,out,profile,control,nativeCall,quotaProfile}){
 const evidence={clock:'real wall clock; production 60-second cadence',foregroundMeaning:'GraphQL approval remains usable while all REST reads are reserved',snapshots:[],calls:[]};
 const save=()=>writeFile(resolve(out,'quota-scenario.json'),JSON.stringify(evidence,null,2));
 const inventory=()=>Promise.all(pages.map(p=>p.evaluate(()=>window.__enterprise.inventory())));
 const until=async(predicate,timeout,label)=>{const end=Date.now()+timeout;while(Date.now()<end){if(await predicate())return;await delay(1000);}throw Error('quota timeout: '+label);};
 try{
  await control('start');
  await until(async()=>{const values=await inventory();return values.every(i=>i.reviewing.length>0)&&provider.ledger.some(r=>r.bucket==='rest'&&r.quota?.before>500&&r.quota.after<=500);},60000,'initial partial inventory and REST reserve');
  const reserve=provider.ledger.find(r=>r.bucket==='rest'&&r.quota.before>500&&r.quota.after<=500);
  evidence.reserve={receiptId:reserve.id,admission:(await control('status')).admission};await save();
  const p=pages[0];await p.getByRole('button',{name:/Synthetic review 51(?:\D|$)/}).first().click();
  await p.getByText('Synthetic description 51',{exact:false}).waitFor();
  await p.getByRole('button',{name:'Approve',exact:true}).first().click();
  await p.getByRole('button',{name:'Approved',exact:true}).first().waitFor();
  const write=provider.ledger.find(r=>r.operation==='review-write'&&r.status===200);
  evidence.foreground={status:write?.status,receiptId:write?.id,role:'desktop',at:Date.now(),admission:(await control('status')).admission};
  const before=await inventory();await save();
  // Each call is an actual foreground detail acquisition, not a provider fetch
  // or scheduler wake. Bound work and stop immediately at the first denial.
  for(let n=0;n<100&&!provider.ledger.some(r=>r.quota?.naturalLimit);n++){
   const number=52+n%100,row=provider.data.rows.find(r=>r.number===number),role=n%2?'paired':'desktop';
   const response=await nativeCall(role,'get_pr_detail',{repo:row.repository.nameWithOwner,number});
   evidence.calls.push({role,number,status:response.status,body:await response.json()});
  }
  const exhausted=provider.ledger.find(r=>r.quota?.naturalLimit);assert.ok(exhausted,'bounded foreground workload naturally exhausts quota');
  const markers=await Promise.all(pages.map(p=>p.evaluate(()=>performance.now())));
  evidence.exhaustion={receiptId:exhausted.id,admission:(await control('status')).admission};await save();
  await delay(2000);const start=Date.now(),offset=provider.ledger.length;
  assert.ok(exhausted.quota.reset*1000-start>12000,'blocked interval before real reset required');
  await delay(12000);
  const blocked=provider.ledger.slice(offset);evidence.quiescence={durationMs:Date.now()-start,restRequests:blocked.filter(r=>r.bucket==='rest').length,graphqlRequests:blocked.filter(r=>r.bucket==='graphql').length};
  const blockedPublication=(page,i)=>page.evaluate(marker=>window.__enterprise.telemetry.filter(e=>e.kind==='event'&&e.name==='source-poll-status'&&e.at>marker&&e.reply?.list==='reviewing'&&['failed','not_asked','retrying','partial','unknown'].includes(e.reply.phase)).at(-1)?.reply,markers[i]);
  await until(async()=>(await Promise.all(pages.map(blockedPublication))).every(Boolean),90000,'actual quota-blocked background source refresh');
  evidence.blockedPublications=await Promise.all(pages.map(blockedPublication));
  const after=await inventory();evidence.retained=before.map((value,i)=>({role:i?'paired':'desktop',before:value,after:after[i]}));await save();
  // Passive observation only from here: no refreshQueues, refreshDetail, wake,
  // artificial clock, changing profile, or control request that triggers work.
  await until(()=>provider.ledger.some(r=>r.bucket==='graphql'&&r.status===200&&r.quota.windowId!==exhausted.quota.windowId&&r.quota.receivedAt>=exhausted.quota.reset*1000),240000,'autonomous real-clock recovery');
  await until(async()=>(await inventory()).every(i=>i.authored.length===50&&i.reviewing.length===150),60000,'mounted recovered inventories');
  const recovered=provider.ledger.find(r=>r.bucket==='graphql'&&r.status===200&&r.quota.windowId!==exhausted.quota.windowId&&r.quota.receivedAt>=exhausted.quota.reset*1000);
  const publication=page=>page.evaluate(reset=>{const source=window.__enterprise.sourceEvidence();const publication=window.__enterprise.telemetry.filter(e=>e.kind==='event'&&e.name==='source-poll-status'&&e.reply?.list==='reviewing'&&e.reply.phase==='ready'&&e.reply.coverage==='complete'&&Date.parse(e.reply.last_received_at)>=reset&&e.reply.last_received_at===source?.lastReceivedAt).at(-1)?.reply;return {source,publication};},exhausted.quota.reset*1000);
  await until(async()=>(await Promise.all(pages.map(publication))).every(v=>v.publication),90000,'accepted recovered source consumed by both mounted roles');
  const published=await Promise.all(pages.map(publication));
  const events=(await readFile(resolve(profile,'native.ndjson'),'utf8')).trim().split('\n').map(JSON.parse);
  const accepted=events.filter(e=>e.operation==='scan-slot'&&e.stage==='accepted-reviewing'&&published.some(p=>p.publication.receipt_revision===e.code));assert.ok(accepted.length,'native accepted revision joins mounted source publications');
  evidence.recovery={receiptId:recovered.id,autonomous:true,accepted,publications:published.map(v=>v.publication),sources:published.map(v=>v.source),native:(await control('status')).admission,inventories:await inventory()};
  result.quotaAccounting=assertQuotaScenario(evidence,provider.ledger,quotaProfile);
  for(const page of pages)result.expectedFailures.push(...await page.evaluate(()=>window.__enterprise.telemetry.filter(e=>e.kind==='call'&&e.ok===false&&['get_pr_detail','get_reviewing','refresh_now','get_review_gates'].includes(e.name))));
  result.phases.push({name:'natural-weighted-quota-reserve-exhaustion-autonomous-recovery',applied:true,exercised:true,converged:true});
 }finally{await save();}
}
