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
