import test from 'node:test';
import assert from 'node:assert/strict';
import {assertConsumption,assertRecovery} from './convergence-consumption.mjs';
const fixture=()=>({requestId:'synthetic:1',commandId:10,held:{id:1,released:true,responseDelivered:true},lineage:{providerId:1,commandId:10,slotId:20,readId:30},releasedAt:1000,completion:{completedWallTime:1001,ok:true,callId:10,args:{requestId:'synthetic:1'},reply:{request_id:'synthetic:1',update:{list:'reviewing',prs:[{repo:'r',number:1}],phase:'ready',receipt_revision:7,last_received_at:'2026-10-05T00:00:00Z'}}},expected:{authored:[],reviewing:['r/1'],ready:['r/1']},clients:[{inventory:{authored:[],reviewing:['r/1']},ready:['r/1']},{inventory:{authored:[],reviewing:['r/1']},ready:['r/1']}],elapsedMs:9000,ceilingMs:10000});
test('delivered HTTP and correct visible cache cannot replace exact native completion',()=>{const f=fixture();delete f.completion;assert.throws(()=>assertConsumption(f));});
test('unrelated command or provider lineage cannot satisfy consumption',()=>{for(const field of ['commandId','providerId']){const f=fixture();f.lineage[field]++;assert.throws(()=>assertConsumption(f));}});
test('matched retained winner and named in-band CAS preserve timestamp and both clients',()=>{for(const error of [null,'The queue changed during this step. Its newer progress was retained.']){const f=fixture();f.completion.reply.update.phase=error?'failed':'ready';f.completion.reply.update.error=error;assertConsumption(f);}});
test('transport refusal, unknown source failure, changed rows, or late completion fail',()=>{for(const mutate of [f=>f.completion.ok=false,f=>f.completion.reply.update.error='unknown',f=>f.clients[1].ready=[],f=>f.completion.reply.update.prs=[],f=>f.elapsedMs=10001]){const f=fixture();mutate(f);assert.throws(()=>assertConsumption(f));}});
test('consumption alone does not prove subsequent fresh traversal',()=>{const f=fixture();assertConsumption(f);const recovery={releasedAt:1000,elapsedMs:180000,ceilingMs:180000,provider:[{at:999,status:200}],clients:[{source:{phase:'ready',coverage:'complete',lastReceivedAt:new Date(999).toISOString()}}]};assert.throws(()=>assertRecovery(recovery));recovery.provider[0].at=1001;recovery.clients[0].source.lastReceivedAt=new Date(1001).toISOString();assertRecovery(recovery);recovery.elapsedMs++;assert.throws(()=>assertRecovery(recovery));});
test('real lineage rejects unrelated slot, HTTP, and incomplete native scope',async()=>{
 const {consumptionLineage}=await import('./convergence-consumption.mjs');
 const events=[{operation:'command-scan',stage:'slot',id:10,code:20},{operation:'read-submitted',stage:'scan-slot',id:30,code:20},{operation:'read-submitted',stage:'synthetic-ledger',id:30,code:1},{operation:'read-submitted',stage:'body-complete',id:30,ns:50},{operation:'command',stage:'complete',id:10,ns:60}];
 assert.deepEqual(consumptionLineage(events,10,1),{commandId:10,providerId:1,readId:30,slotId:20});
 for(const index of [0,1,2,3,4])assert.throws(()=>consumptionLineage(events.filter((_,i)=>i!==index),10,1));
 const wrong=structuredClone(events);wrong[1].code=21;assert.throws(()=>consumptionLineage(wrong,10,1));
});
