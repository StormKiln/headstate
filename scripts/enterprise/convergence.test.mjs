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

test('correct visible cache cannot satisfy withheld native stale-page completion',async()=>{
 const {releaseWithPublication}=await import('./convergence.mjs');const telemetry=[];
 const page={evaluate:async(fn,args)=>{globalThis.window={__enterprise:{telemetry}};return fn(args);},waitForFunction:async(fn,args)=>{globalThis.window={__enterprise:{telemetry}};if(!fn(args))throw Error('native completion withheld');}};
 const entry={held:true,released:false,responseDelivered:false};const provider={ledger:[entry],release(){entry.released=true;entry.responseDelivered=true;}};
 await assert.rejects(releaseWithPublication({pages:[page],provider,ceilingMs:1}),/completion withheld/);
 entry.released=false;provider.release=()=>{entry.released=true;entry.responseDelivered=true;telemetry.push({kind:'event',name:'source-poll-status',reply:{list:'reviewing',prs:[],receipt_revision:2,last_received_at:new Date(Date.now()+1).toISOString()}});};
 assert.equal((await releaseWithPublication({pages:[page],provider})).publications.length,1);delete globalThis.window;
});
test('detail acquisition guard ignores explicit continuations but rejects same-identity loops',async()=>{
 const {assertPrimaryDetailCount}=await import('./convergence.mjs');const primary={primaryDetails:[{repo:'synthetic-lab/repo-1',number:51}]};
 assert.equal(assertPrimaryDetailCount([primary,{queryName:'ChecksContinuation'},{operation:'search'}],51,1).length,1);
 assert.throws(()=>assertPrimaryDetailCount([primary,primary],51,1),/primary detail acquisitions/);
 assertPrimaryDetailCount([{primaryDetails:[{repo:'other/repo',number:51}]}],51,0);
});
