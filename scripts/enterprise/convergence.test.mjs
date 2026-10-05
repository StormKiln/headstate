import {test} from 'node:test';import assert from 'node:assert/strict';
import {initialConvergenceExpected,changeExpected,assertConverged,convergencePolicy} from './convergence.mjs';
test('predeclared convergence identities are independent and exact',()=>{
 const expected=initialConvergenceExpected();assert.equal(expected.authored.length,50);assert.equal(expected.reviewing.length,236);assert.equal(expected.ready.length,130);
 assert.equal(expected.authored.filter(id=>expected.reviewing.includes(id)).length,0);
 assert.equal(convergencePolicy.inventoryMs,180000);assert.equal(convergencePolicy.localPropagationMs,10000);
});
test('explicit external transitions have the declared 234/126 result and inverses',()=>{
 const expected=initialConvergenceExpected();
 for(const number of [51,52])changeExpected(expected,number,{member:true,ready:false},'observed readiness change');
 for(const number of [53,54])changeExpected(expected,number,{member:false,ready:false},'observed terminal');
 assert.equal(expected.reviewing.length,234);assert.equal(expected.ready.length,126);
 changeExpected(expected,55,{member:true,ready:false},'enqueue');changeExpected(expected,55,{member:true,ready:true},'dequeue');assert.equal(expected.ready.length,126);
 assertConverged({inventory:{authored:expected.authored,reviewing:expected.reviewing},ready:expected.ready},expected);
 assert.throws(()=>assertConverged({inventory:{authored:expected.authored,reviewing:expected.reviewing},ready:expected.ready.slice(1)},expected));
 assert.throws(()=>changeExpected(expected,53,{member:false,ready:true},'invalid oracle'));
});
