import test from 'node:test';import assert from 'node:assert/strict';
import {prepareActionThenHold} from './convergence-action.mjs';
test('actual action readiness precedes held acquisition and is rechecked afterward',async()=>{
 const order=[];let ready=false;
 const preparation=await prepareActionThenHold({prepare:async()=>{order.push('open');await Promise.resolve();ready=true;order.push('enabled');return {guidanceMs:30000};},hold:async()=>{assert.equal(ready,true);order.push('hold');},verify:async()=>{assert.equal(ready,true);order.push('verify');}});
 assert.deepEqual(order,['open','enabled','hold','verify']);assert.equal(preparation.guidanceMs,30000);
});
test('failed prerequisite never arms hold and lost readiness cannot proceed to action',async()=>{
 let held=false,acted=false;
 await assert.rejects(prepareActionThenHold({prepare:async()=>{throw Error('not enabled');},hold:async()=>{held=true;},verify:async()=>{}}));assert.equal(held,false);
 await assert.rejects((async()=>{await prepareActionThenHold({prepare:async()=>{},hold:async()=>{held=true;},verify:async()=>{throw Error('readiness expired');}});acted=true;})());assert.equal(held,true);assert.equal(acted,false);
});
test('hold candidates need actual enabled control and sufficient unexpired evidence',async()=>{
 const {holdReadiness,observeHoldReadiness}=await import('./convergence-action.mjs');
 const observation={enabled:true,at:100,stacks:[{fresh:true,expiresAt:15100}]};assert.equal(holdReadiness(observation).eligible,true);
 for(const patch of [{enabled:false},{at:101},{stacks:[]},{stacks:[{fresh:false,expiresAt:99999}]},{stacks:[{fresh:true,expiresAt:99}]}])assert.equal(holdReadiness({...observation,...patch}).eligible,false);
 assert.equal((await observeHoldReadiness(()=>new Promise(()=>{}),1)).eligible,false);assert.equal((await observeHoldReadiness(()=>{throw Error('closed');})).eligible,false);
});
