import assert from 'node:assert/strict';
import test from 'node:test';
import {fixture} from './provider.mjs';
import {remainingWork,diagnosticCeiling,assertAdvisoryPeriods,displayEvidence,verifiedApprovedIdentities} from './final-acceptance.mjs';
test('remaining work combines independently witnessed rules and pusher without charging proved work twice',()=>{
 const remaining=remainingWork({pushers:[51,52],rules:[51,53],stacks:[51,52,53]},[51,52,53]);
 assert.deepEqual(remaining,{pushers:[52,53],stacks:[]});assert.equal(diagnosticCeiling(remaining),1800000);
 assert.equal(diagnosticCeiling({pushers:Array(148),stacks:Array(148)}),304*60000);
});
test('actual per-principal debits cannot exceed reserved share or global period cap',()=>{
 assertAdvisoryPeriods([{allocations:{0:4,1:4},debits:{0:4,1:4}}]);
 assert.throws(()=>assertAdvisoryPeriods([{allocations:{0:4,1:4},debits:{0:5}}]));
 assert.throws(()=>assertAdvisoryPeriods([{allocations:{0:9},debits:{0:9}}]));
});
test('currently fresh, retained and unresolved identity sets remain distinct from ever-useful coverage',()=>{
 const q=[{kind:'ready-pushers',syntheticNumber:51,evidence:{pusher:{fresh:false},rules:{fresh:true}},measuredPusher:true,measuredRules:true}];
 const result=displayEvidence(q,[51,52]);assert.deepEqual(result.pushers,{fresh:[],retained:[51],unresolved:[52]});assert.deepEqual(result.rules.fresh,[51]);assert.deepEqual(result.stacks.unresolved,[51,52]);
});

test('eligible exclusions require exactly three independently successful current-head approvals',()=>{
 const rows=fixture().rows;
 for(const number of [51,52,53]){const row=rows.find(r=>r.number===number);row.reviews.nodes=[{author:{login:'synthetic-viewer'},state:'APPROVED',commit:{oid:row.headRefOid}}];}
 const ledger=Array.from({length:3},()=>({operation:'review-write',status:200,terminal:'response-sent'}));
 assert.deepEqual(verifiedApprovedIdentities(rows,ledger),['synthetic-lab/repo-1/51','synthetic-lab/repo-2/52','synthetic-lab/repo-3/53']);
 assert.throws(()=>verifiedApprovedIdentities(rows,ledger.slice(1)));
 assert.throws(()=>verifiedApprovedIdentities(rows,ledger.map((e,i)=>i?e:{...e,status:503})));
 const row=rows.find(r=>r.number===53);row.reviews.nodes[0].commit.oid='older-head';assert.throws(()=>verifiedApprovedIdentities(rows,ledger));
 row.reviews.nodes[0].commit.oid=row.headRefOid;
 rows.find(r=>r.number===54).reviews.nodes=[{author:{login:'synthetic-viewer'},state:'APPROVED',commit:{oid:'head-54'}}];assert.throws(()=>verifiedApprovedIdentities(rows,ledger));
});

test('fixed continuing145 population cannot waive a missing useful identity',async()=>{
 const {usefulPublications,assertCompletionDeadline}=await import('./ready-completion.mjs');
 const expected=Array.from({length:145},(_,i)=>54+i);
 const receipts=expected.map(number=>({number,freshPusher:true,freshRules:true,freshStack:true}));
 assert.doesNotThrow(()=>assertCompletionDeadline(usefulPublications(receipts,expected),expected,1800000,diagnosticCeiling({pushers:expected,stacks:expected})));
 assert.throws(()=>assertCompletionDeadline(usefulPublications(receipts.slice(1),expected),expected,1800000,diagnosticCeiling({pushers:expected,stacks:expected})));
 assert.deepEqual(remainingWork(usefulPublications(receipts.slice(1),expected),expected),{pushers:[54],stacks:[54]});
});
