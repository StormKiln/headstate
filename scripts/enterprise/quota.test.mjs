import {test} from 'node:test';
import assert from 'node:assert/strict';
import {parseDocument} from './graphql.mjs';
import {weightedQuotaProfile,validateQuotaProfile,quotaIdentity,createQuotaState,documentCost,reconcileQuota} from './quota.mjs';
const doc=q=>parseDocument(q);
const profile=()=>structuredClone(weightedQuotaProfile);
test('shape costs are deterministic and alias/page/mutation sensitive',()=>{
 const p=profile();
 const small=doc('{search(first:25,query:"is:open"){nodes{id}}}');
 const large=doc('{search(first:100,query:"is:open"){nodes{id}}}');
 assert.ok(documentCost(p,{doc:large})>documentCost(p,{doc:small}));
 assert.ok(documentCost(p,{doc:doc('mutation {closePullRequest(input:{pullRequestId:"PR_1"}){pullRequest{id}}}')})>documentCost(p,{doc:small}));
 assert.equal(documentCost(p,{doc:small}),documentCost(p,{doc:small}));
 assert.ok(documentCost(p,{doc:doc('{a:node(id:"PR_1"){id} b:node(id:"PR_2"){id}}')})>documentCost(p,{doc:doc('{node(id:"PR_1"){id}}')}));
 assert.notEqual(documentCost(p,{method:'GET',path:'/repos/a/b/activity'}),documentCost(p,{method:'GET',path:'/repos/a/b/protection'}));
});
test('independent buckets reset with exact and over-cost boundaries; receipts replay',()=>{
 const p=profile();p.graphql={capacity:10,windowSeconds:130};p.rest={capacity:20,windowSeconds:180};p.burst.maxCost=10000;
 const s=createQuotaState(p,1000);const rows=[];
 rows.push(s.charge({bucket:'graphql',cost:10,now:1001}));
 rows.push(s.charge({bucket:'graphql',cost:1,now:1002}));
 rows.push(s.charge({bucket:'rest',cost:21,now:1003}));
 assert.deepEqual(rows.map(r=>[r.before,r.charged,r.after,r.naturalLimit]),[[10,10,0,false],[0,0,0,true],[20,0,20,true]]);
 rows.push(s.charge({bucket:'graphql',cost:2,now:1130001}));
 assert.equal(rows.at(-1).before,10);
 assert.equal(reconcileQuota(p,rows).charged.graphql,12);
 for(const field of ['cost','charged','after','windowId','reset']){const bad=structuredClone(rows);bad[0][field]=field==='windowId'?'forged':999;assert.throws(()=>reconcileQuota(p,bad),undefined,field);}
});
test('burst cooldown charges no rejected requests and expires on clock',()=>{
 const p=profile();p.burst={windowSeconds:2,maxCost:6,cooldownSeconds:3};const s=createQuotaState(p,0);
 assert.equal(s.charge({bucket:'graphql',cost:6,now:0}).secondaryLimit,false);
 const denied=s.charge({bucket:'rest',cost:1,now:1});assert.equal(denied.secondaryLimit,true);assert.equal(denied.charged,0);assert.equal(denied.retryAfter,3);
 assert.equal(s.charge({bucket:'rest',cost:1,now:3000}).secondaryLimit,true);
 assert.equal(s.charge({bucket:'rest',cost:1,now:3001}).secondaryLimit,false);
});
test('profile rejects invalid parameters and identity hashes all parameters',()=>{
 for(const [key,value] of [['capacity',0],['capacity',Infinity],['windowSeconds',0]]){const p=profile();p.rest[key]=value;assert.throws(()=>validateQuotaProfile(p));}
 const p=profile();p.costs.mutation=-1;assert.throws(()=>validateQuotaProfile(p));
 const p2=profile();p2.rest.capacity++;assert.notEqual(quotaIdentity(profile()).hash,quotaIdentity(p2).hash);
});

test('reset boundaries refill only the due bucket and receipt snapshots remain immutable',()=>{
 const p=profile();p.graphql={capacity:10,windowSeconds:130};p.rest={capacity:20,windowSeconds:180};const state=createQuotaState(p,0);
 const first=state.charge({bucket:'graphql',cost:5,now:0});state.charge({bucket:'rest',cost:5,now:0});
 assert.equal(state.charge({bucket:'graphql',cost:1,now:129999}).before,5);
 assert.equal(state.charge({bucket:'graphql',cost:1,now:130000}).before,10);
 assert.equal(state.charge({bucket:'rest',cost:1,now:130000}).before,15);
 assert.equal(state.charge({bucket:'rest',cost:1,now:180000}).before,20);
 assert.equal(first.after,5);assert.throws(()=>{first.after=0;});
});
