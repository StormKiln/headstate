import assert from 'node:assert/strict';
import test from 'node:test';
import {fixture} from './provider.mjs';
import {remainingWork,diagnosticCeiling,assertStatsMeasurement,assertAdvisoryPeriods,displayEvidence} from './final-acceptance.mjs';
test('remaining work combines independently witnessed rules and pusher without charging proved work twice',()=>{
 const remaining=remainingWork({pushers:[51,52],rules:[51,53],stacks:[51,52,53]},[51,52,53]);
 assert.deepEqual(remaining,{pushers:[52,53],stacks:[]});assert.equal(diagnosticCeiling(remaining),1800000);
 assert.equal(diagnosticCeiling({pushers:Array(148),stacks:Array(148)}),304*60000);
});
test('exact Stats DOM and author/repository aggregates reject missing identities and invented zero denominator',()=>{
 const history=fixture().rows.filter(r=>r.state==='MERGED');const repos=new Map(),authors=new Map();
 for(const r of history){repos.set(r.repository.nameWithOwner,(repos.get(r.repository.nameWithOwner)??0)+1);authors.set(r.author.login,(authors.get(r.author.login)??0)+1);}
 const today=new Date();today.setUTCHours(0,0,0,0);const window={from:new Date(today.getTime()-30*86400000).toISOString().slice(0,10),to:new Date(today.getTime()-86400000).toISOString().slice(0,10)};
 const board={window,complete:true,accumulated:250,total:250,days:30,daysCovered:30,daysTotal:30,owner:{viewer:'synthetic-viewer'},scopeKey:'merged|*|org:synthetic-lab',repoCounts:[...repos].map(([repo,merged])=>({repo,merged})),rows:[...authors].map(([login,prs])=>({login,prs}))};
 const dom={repos:[...repos].map(([repo,count])=>({repo,count,share:'2%'})),completeHint:true,impossibleHint:false};
 assertStatsMeasurement(board,dom,history);
 assert.throws(()=>assertStatsMeasurement({...board,window:{...window,from:window.to}},dom,history));
 assert.throws(()=>assertStatsMeasurement({...board,total:0},dom,history));
 assert.throws(()=>assertStatsMeasurement(board,{...dom,repos:dom.repos.slice(1)},history));
 assert.throws(()=>assertStatsMeasurement(board,{...dom,repos:dom.repos.map((r,i)=>i? r:{...r,share:'3%'})},history));
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
