import {test} from 'node:test';
import assert from 'node:assert/strict';
import {assertQuotaScenario} from './quota-scenario.mjs';
import {weightedQuotaProfile,createQuotaState,documentCost} from './quota.mjs';
import {parseDocument} from './graphql.mjs';
function valid(){
 const p=structuredClone(weightedQuotaProfile);p.graphql.capacity=100;p.burst.maxCost=10000;
 const state=createQuotaState(p,0),ledger=[];
 const add=(shape,now,operation)=>{const bucket=shape.doc?'graphql':'rest',quota=state.charge({bucket,cost:documentCost(p,shape),now});ledger.push({id:ledger.length+1,bucket,quota,quotaShape:shape,fault:'healthy',status:quota.naturalLimit?403:200,operation});return ledger.at(-1);};
 for(let i=0;i<4;i++)add({method:'GET',path:'/repos/a/b/activity'},i,'rest-read');
 const write=add({doc:parseDocument('mutation {closePullRequest(input:{pullRequestId:"PR_1"}){pullRequest{id}}}')},10,'review-write');
 const exhausted=add({doc:parseDocument('{repository(owner:"a",name:"b"){pullRequest(number:1){body reviewThreads{nodes{id}}}}}')},20,'repository');
 const recovered=add({doc:parseDocument('{viewer{login}}')},151000,'viewer');
 const inv={authored:['a'],reviewing:['synthetic-lab/repo-1/51']};
 const blocked={session:'s',list:'reviewing',receipt_revision:1,phase:'failed',coverage:'partial',prs:[{repo:'synthetic-lab/repo-1',number:51,head_oid:'head-51',state:'open',is_draft:false,base_ref:'main'}],last_received_at:'1970-01-01T00:00:00.000Z'};const publication={...blocked,receipt_revision:2,phase:'ready',coverage:'complete',last_received_at:'1970-01-01T00:02:31.000Z'};
 const e={blockedPublications:[blocked,blocked],reserve:{receiptId:4,admission:{rest:{reserveCooldownMs:1000}}},foreground:{status:200,receiptId:write.id,admission:{rest:{reserveCooldownMs:1000}}},exhaustion:{receiptId:exhausted.id,admission:{graphql:{primaryCooldownMs:1000}}},quiescence:{durationMs:12000,restRequests:0,graphqlRequests:0},retained:[{before:inv,after:inv},{before:inv,after:inv}],recovery:{receiptId:recovered.id,autonomous:true,accepted:[{operation:'scan-slot',stage:'accepted-reviewing',code:2}],publications:[publication,publication],sources:[{phase:'ready',coverage:'complete',lastReceivedAt:publication.last_received_at},{phase:'ready',coverage:'complete',lastReceivedAt:publication.last_received_at}],native:{graphql:{remaining:97}},inventories:[{authored:Array(50).fill('a'),reviewing:Array(150).fill('b')},{authored:Array(50).fill('a'),reviewing:Array(150).fill('b')}]}};
 return {e,ledger,p};
}
test('complete replayable natural quota evidence passes',()=>{const {e,ledger,p}=valid();assert.equal(assertQuotaScenario(e,ledger,p).documents,7);});
test('scenario rejects each missing obligation, forged receipts and fault shortcuts',()=>{
 for(const key of ['reserve','foreground','exhaustion','quiescence','retained','recovery','blockedPublications']){const {e,ledger,p}=valid();delete e[key];assert.throws(()=>assertQuotaScenario(e,ledger,p),undefined,key);}
 for(const edit of [e=>e.recovery.autonomous=false,e=>e.blockedPublications[0].prs[0].head_oid='wrong-head',e=>e.recovery.accepted=[],e=>e.foreground.admission.rest.reserveCooldownMs=0,e=>e.recovery.publications[0].receipt_revision=1,e=>e.recovery.sources[0].lastReceivedAt='stale',e=>e.exhaustion.admission.graphql.primaryCooldownMs=0,e=>e.quiescence.restRequests=1,e=>e.quiescence.graphqlRequests=3,e=>e.retained[0].after={authored:[],reviewing:[]},e=>e.recovery.receiptId=5]){const {e,ledger,p}=valid();edit(e);assert.throws(()=>assertQuotaScenario(e,ledger,p));}
 for(const edit of [r=>r.quota={...r.quota,cost:1},r=>r.quota={...r.quota,after:999},r=>r.quota={...r.quota,windowId:'forged'},r=>r.fault='primary']){const {e,ledger,p}=valid();edit(ledger[0]);assert.throws(()=>assertQuotaScenario(e,ledger,p));}
});
