import test from 'node:test';
import assert from 'node:assert/strict';
import {observeUntil,observeRoles} from './churn.mjs';

test('scheduled wait rejects a transient regression even when endpoint recovers',async()=>{
 let now=0;let reads=0;const endpointHealthy=()=>now!==2000;
 await assert.rejects(observeUntil({deadline:75000,now:()=>now,sleep:async ms=>{now+=ms;},observe:async()=>{reads++;assert.ok(endpointHealthy(),'same-head approved fact regressed');}}),/regressed/);
 assert.equal(now,2000);assert.equal(reads,3);now=75000;assert.equal(endpointHealthy(),true);
});
test('scheduled read-only observations preserve deadline and bounded sampling',async()=>{
 let now=365;const waits=[];let reads=0;
 await observeUntil({deadline:75000,now:()=>now,sleep:async ms=>{waits.push(ms);now+=ms;},observe:async()=>{reads++;}});
 assert.equal(now,75000);assert.equal(reads,76);assert.ok(waits.every(ms=>ms>0&&ms<=1000));
});
test('unconverged first role does not suppress second role regression',async()=>{
 const seen=[];
 await assert.rejects(observeRoles(['desktop','paired'],async role=>{seen.push(role);throw Error(role==='desktop'?'not converged':'accepted fact regression');}),/regression/);
 assert.deepEqual(seen,['desktop','paired']);
});
test('scheduled wait rejects transient mounted draft focus loss',async()=>{
 let now=0;
 await assert.rejects(observeUntil({deadline:5000,now:()=>now,sleep:async ms=>{now+=ms;},observe:async()=>{assert.notEqual(now,1000,'draft focus lost');}}),/focus lost/);
});
