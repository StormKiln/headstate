import test from 'node:test';import assert from 'node:assert/strict';import {assertMixedOverlap} from './mixed-overlap.mjs';
function fixture(){return {events:[{id:10,operation:'corpus-pass',stage:'begin',ns:100},{id:20,operation:'command',stage:'begin',ns:150},{id:10,operation:'corpus-pass',stage:'indexed',ns:190,code:100},{id:10,operation:'corpus-pass',stage:'complete',ns:200},{id:20,operation:'command',stage:'complete',ns:210}],clients:[{role:'desktop',telemetry:[{kind:'call',name:'get_pr_detail',ok:true,callId:20}]}]};}
test('mixed overlap binds real foreground call to same-clock positive-work corpus pass without resources',()=>{const f=fixture();assert.equal(assertMixedOverlap(f.events,f.clients).length,1);});
test('mixed overlap rejects sleep-only timing, zero-work, missing/failed boundaries and foreign calls',()=>{
 for(const corrupt of [f=>f.events[3].ns=140,f=>f.events[2].code=0,f=>f.events.splice(3,1),f=>f.events[3].stage='failed',f=>f.clients[0].telemetry[0].name='claude_coverage',f=>f.clients[0].telemetry[0].ok=false,f=>f.clients[0].telemetry[0].callId=21]){const f=fixture();corrupt(f);assert.throws(()=>assertMixedOverlap(f.events,f.clients));}
});
