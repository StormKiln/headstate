import {test} from 'node:test';
import assert from 'node:assert/strict';
import {compareBaseline} from './budgets.mjs';
const baseline={summaries:[{engine:'webkit',warm:false,role:'desktop',budgets:{firstUsefulQueue:100},documentBudget:4}]};
const result={engine:'webkit',warm:false,samples:[{role:'desktop',firstUsefulQueue:90}],providerReceipts:4};
test('measured baseline accepts bounded samples and rejects timing/document/missing evidence',()=>{
 assert.equal(compareBaseline(result,baseline).length,1);
 assert.throws(()=>compareBaseline({...result,providerReceipts:5},baseline),/documents/);
 assert.throws(()=>compareBaseline({...result,samples:[{role:'desktop',firstUsefulQueue:101}]},baseline),/firstUsefulQueue/);
 assert.throws(()=>compareBaseline({...result,samples:[{role:'desktop'}]},baseline),/firstUsefulQueue/);
 assert.throws(()=>compareBaseline({...result,engine:'chromium'},baseline),/baseline row/);
});
