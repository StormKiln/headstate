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
export function assertRecovery({releasedAt,elapsedMs,ceilingMs,provider,clients}){
 assert.ok(elapsedMs<=ceilingMs,'fresh traversal exceeded original recovery ceiling');
 assert.ok(provider.some(entry=>entry.at>releasedAt&&entry.status===200),'no new successful provider work');
 for(const {source} of clients){assert.equal(source.phase,'ready');assert.equal(source.coverage,'complete');assert.ok(Date.parse(source.lastReceivedAt)>releasedAt,'preexisting winner is not recovery');}
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
