import assert from 'node:assert/strict';
import {assertConverged} from './convergence.mjs';
const cas='The queue changed during this step. Its newer progress was retained.';
export function assertConsumption({requestId,commandId,held,lineage,completion,expected,clients,elapsedMs,ceilingMs,releasedAt}){
 assert.ok(elapsedMs<=ceilingMs,'native consumption exceeded local ceiling');
 assert.ok(held.released&&held.responseDelivered,'original held response was not delivered');
 assert.equal(lineage.providerId,held.id);assert.equal(lineage.commandId,commandId);
 assert.ok(Number.isInteger(lineage.slotId)&&Number.isInteger(lineage.readId));
 assert.equal(completion?.ok,true,'exact native command must complete successfully');assert.ok(completion.completedWallTime>=releasedAt,'command completion preceded release');assert.equal(completion.callId,commandId);
 assert.equal(completion.args.requestId,requestId);assert.equal(completion.reply.request_id,requestId);
 const update=completion.reply.update;assert.equal(update.list,'reviewing');assert.ok(update.receipt_revision!=null);
 assert.deepEqual(update.prs.map(row=>`${row.repo}/${row.number}`).sort(),expected.reviewing);
 assert.ok(!update.error||update.error===cas,'unclassified source error');
 for(const client of clients)assertConverged(client,expected);
 return {classification:update.error===cas?'matched-cas-refusal':'matched-row-bearing-completion',completion,lineage};
}
export function assertRecovery({releasedAt,releasedMono,elapsedMs,ceilingMs,provider,clients,events,consumed}){
 assert.ok(elapsedMs<=ceilingMs,'fresh traversal exceeded original recovery ceiling');
 const candidates=events.filter(e=>e.operation==='scan-slot'&&e.stage==='accepted-reviewing'&&e.code>consumed.receipt_revision);
 for(const accepted of candidates){
  const read=events.find(e=>e.operation==='read-submitted'&&e.stage==='scan-slot'&&e.code===accepted.id&&events.some(body=>body.id===e.id&&body.operation==='read-submitted'&&body.stage==='body-complete'&&body.ns<=accepted.ns)&&events.some(link=>link.id===e.id&&link.stage==='synthetic-ledger'&&provider.some(p=>p.id===link.code&&p.at>releasedMono&&p.status===200&&p.responseDelivered)));
  if(!read)continue;
  const publications=clients.map(client=>client.publications.find(p=>p.session===consumed.session&&p.list==='reviewing'&&p.receipt_revision===accepted.code&&p.phase==='ready'&&p.coverage==='complete'&&Date.parse(p.last_received_at)>Math.max(releasedAt,Date.parse(consumed.last_received_at))&&client.source.phase==='ready'&&client.source.coverage==='complete'&&client.source.lastReceivedAt===p.last_received_at));
  if(publications.every(Boolean))return {accepted,read,publications};
 }
 assert.fail('no later accepted traversal with exact post-release body and both client receipts');
}
// Each edge is emitted at the actual caller registration / producer request / response boundary.
export function consumptionLineage(events,commandId,providerId){
 const slots=new Set(events.filter(e=>e.operation==='command-scan'&&e.stage==='slot'&&e.id===commandId).map(e=>e.code));
 const reads=events.filter(e=>e.operation==='read-submitted'&&e.stage==='synthetic-ledger'&&e.code===providerId);
 const chains=reads.flatMap(read=>events.filter(e=>e.id===read.id&&e.operation==='read-submitted'&&e.stage==='scan-slot'&&slots.has(e.code)).map(e=>({commandId,providerId,readId:read.id,slotId:e.code})));
 assert.equal(chains.length,1,'held response requires one exact command/slot/HTTP lineage');
 const complete=events.find(e=>e.id===commandId&&e.operation==='command'&&e.stage==='complete');
 const body=events.find(e=>e.id===chains[0].readId&&e.operation==='read-submitted'&&e.stage==='body-complete');
 assert.ok(complete&&body&&body.ns<=complete.ns,'exact held body must complete before native command');
 return chains[0];
}
