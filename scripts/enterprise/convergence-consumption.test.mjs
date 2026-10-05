import test from 'node:test';
import assert from 'node:assert/strict';
import {assertConsumption,assertRecovery} from './convergence-consumption.mjs';
const fixture=()=>({requestId:'synthetic:1',commandId:10,held:{id:1,released:true,responseDelivered:true},lineage:{providerId:1,commandId:10,slotId:20,readId:30},releasedAt:1000,completion:{completedWallTime:1001,ok:true,callId:10,args:{requestId:'synthetic:1'},reply:{request_id:'synthetic:1',update:{list:'reviewing',prs:[{repo:'r',number:1}],phase:'ready',receipt_revision:7,last_received_at:'2026-10-05T00:00:00Z'}}},expected:{authored:[],reviewing:['r/1'],ready:['r/1']},clients:[{inventory:{authored:[],reviewing:['r/1']},ready:['r/1']},{inventory:{authored:[],reviewing:['r/1']},ready:['r/1']}],elapsedMs:9000,ceilingMs:10000});
test('delivered HTTP and correct visible cache cannot replace exact native completion',()=>{const f=fixture();delete f.completion;assert.throws(()=>assertConsumption(f));});
test('unrelated command or provider lineage cannot satisfy consumption',()=>{for(const field of ['commandId','providerId']){const f=fixture();f.lineage[field]++;assert.throws(()=>assertConsumption(f));}});
test('matched retained winner and named in-band CAS preserve timestamp and both clients',()=>{for(const error of [null,'The queue changed during this step. Its newer progress was retained.']){const f=fixture();f.completion.reply.update.phase=error?'failed':'ready';f.completion.reply.update.error=error;assertConsumption(f);}});
test('transport refusal, unknown source failure, changed rows, or late completion fail',()=>{for(const mutate of [f=>f.completion.ok=false,f=>f.completion.reply.update.error='unknown',f=>f.clients[1].ready=[],f=>f.completion.reply.update.prs=[],f=>f.elapsedMs=10001]){const f=fixture();mutate(f);assert.throws(()=>assertConsumption(f));}});
const recoveryFixture=()=>({releasedAt:1000,releasedMono:1000,elapsedMs:100,ceilingMs:180000,consumed:{session:'session1',receipt_revision:7,last_received_at:new Date(1001).toISOString()},provider:[{id:2,at:1001,status:200,responseDelivered:true}],events:[{id:40,operation:'read-submitted',stage:'scan-slot',code:21,ns:50},{id:40,operation:'read-submitted',stage:'synthetic-ledger',code:2,ns:60},{id:40,operation:'read-submitted',stage:'body-complete',ns:70},{id:21,operation:'scan-slot',stage:'accepted-reviewing',code:8,ns:80}],clients:[0,1].map(()=>({source:{phase:'ready',coverage:'complete',lastReceivedAt:new Date(1002).toISOString()},publications:[{session:'session1',list:'reviewing',phase:'ready',coverage:'complete',receipt_revision:8,last_received_at:new Date(1002).toISOString()}]}))});
test('recovery rejects held receipt plus unrelated, pending or refused newer work',()=>{
 for(const mutate of [f=>f.events=[],f=>f.clients[0].source.phase='fetching',f=>f.clients[1].publications[0].phase='fetching',f=>f.events[0].code=999,f=>f.events.splice(2,1),f=>f.events[2].stage='failed',f=>f.events[3].code=7,f=>f.provider[0].at=999,f=>f.clients[1].publications=[],f=>f.clients[1].publications[0].session='other',f=>f.elapsedMs=180001]){const f=recoveryFixture();mutate(f);assert.throws(()=>assertRecovery(f));}
 assertRecovery(recoveryFixture());
});
test('real lineage rejects unrelated slot, HTTP, and incomplete native scope',async()=>{
 const {consumptionLineage}=await import('./convergence-consumption.mjs');
 const events=[{operation:'command-scan',stage:'slot',id:10,code:20},{operation:'read-submitted',stage:'scan-slot',id:30,code:20},{operation:'read-submitted',stage:'synthetic-ledger',id:30,code:1},{operation:'read-submitted',stage:'body-complete',id:30,ns:50},{operation:'command',stage:'complete',id:10,ns:60}];
 assert.deepEqual(consumptionLineage(events,10,1),{commandId:10,providerId:1,readId:30,slotId:20});
 for(const index of [0,1,2,3,4])assert.throws(()=>consumptionLineage(events.filter((_,i)=>i!==index),10,1));
 const wrong=structuredClone(events);wrong[1].code=21;assert.throws(()=>consumptionLineage(wrong,10,1));
});
