import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {resolve} from 'node:path';
const finished=new Set(['body-complete','failed','cancelled','refused-body']);
export function pendingScanSlot(events){
 const slots=new Set(events.filter(e=>e.operation==='read-submitted'&&e.stage==='scan-slot'&&!events.some(end=>end.id===e.id&&finished.has(end.stage))).map(e=>e.code));
 assert.equal(slots.size,1,'setup requires exactly one observable pending scan slot');
 return [...slots][0];
}
export function joinedCommand(before,after,slotId){
 const oldCommands=new Set(before.filter(e=>e.operation==='command').map(e=>e.id));
 const joins=after.filter(e=>e.operation==='command-scan'&&e.stage==='slot'&&e.code===slotId&&!oldCommands.has(e.id));
 assert.equal(joins.length,1,'one new command must join original pending slot');
 const commandId=joins[0].id;
 assert.ok(after.some(e=>e.id===commandId&&e.operation==='command'&&e.stage==='begin'&&e.code===0),'join must belong to desktop command');
 assert.ok(!after.some(e=>e.id===commandId&&e.operation==='command'&&e.stage==='complete'),'joined command must still await held response');
 assert.equal(pendingScanSlot(after),slotId,'held slot must remain pending before effects');
 return commandId;
}
export async function joinAfterHeld({waitForHeld,readEvents,refresh,waitForRegistration}){
 await waitForHeld();
 const before=await readEvents(),slotId=pendingScanSlot(before);
 const requestId=await refresh();assert.equal(typeof requestId,'string');
 const commandId=await waitForRegistration(before,slotId);
 return {requestId,commandId,slotId};
}

export async function joinHeldReviewing({page,provider,profile,inventoryMs=180000,joinMs=10000}){
 const wait=async(test,limit,message)=>{const start=performance.now();while(performance.now()-start<=limit){if(await test()){assert.ok(performance.now()-start<=limit,message);return;}await new Promise(r=>setTimeout(r,50));}throw Error(message);};
 const readEvents=async()=>(await readFile(resolve(profile,'native.ndjson'),'utf8')).split('\n').filter(Boolean).map(JSON.parse);
 return joinAfterHeld({
  waitForHeld:async()=>{await wait(()=>provider.heldCount>0,inventoryMs,'no real materialized reviewing search was held');assert.ok(provider.ledger.some(entry=>entry.held&&!entry.released&&entry.searches?.some(search=>search.query.includes('review-requested:@me'))));},
  readEvents,
  refresh:()=>page.evaluate(()=>{const offset=window.__enterprise.telemetry.length;void window.__enterprise.refreshReviewing().catch(()=>{});const started=window.__enterprise.telemetry.slice(offset).filter(e=>e.kind==='call-start'&&e.name==='get_reviewing');if(started.length!==1)throw Error('one actual reviewing invocation required');return started[0].args.requestId;}),
  waitForRegistration:async(before,slot)=>{let command;await wait(async()=>{try{command=joinedCommand(before,await readEvents(),slot);return true;}catch{return false;}},joinMs,'manual refresh did not join actual pending held scan before effects');return command;},
 });
}
