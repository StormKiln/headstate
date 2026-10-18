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
 const key=n=>`synthetic-lab/repo-${(n-1)%50+1}/${n}`;
 const inv={authored:Array.from({length:50},(_,i)=>key(i+1)).sort(),reviewing:Array.from({length:150},(_,i)=>key(i+51)).sort()};
 const rows=Array.from({length:150},(_,i)=>({repo:`synthetic-lab/repo-${i%50+1}`,number:i+51,head_oid:`head-${i+51}`,is_draft:false,base_ref:'main'}));
 const copy=structuredClone;
 const blocked={session:'s',list:'reviewing',receipt_revision:1,phase:'failed',coverage:'partial',prs:rows,last_received_at:'1970-01-01T00:00:00.000Z'};const publication={...blocked,receipt_revision:2,phase:'ready',coverage:'complete',last_received_at:'1970-01-01T00:02:31.000Z'};
 const e={blockedPublications:[copy(blocked),copy(blocked)],reserve:{receiptId:4,admission:{rest:{reserveCooldownMs:1000}}},foreground:{status:200,receiptId:write.id,admission:{rest:{reserveCooldownMs:1000}}},exhaustion:{receiptId:exhausted.id,admission:{graphql:{primaryCooldownMs:1000}}},quiescence:{durationMs:12000,restRequests:0,graphqlRequests:0},retained:[{before:copy(inv),after:copy(inv)},{before:copy(inv),after:copy(inv)}],recovery:{receiptId:recovered.id,autonomous:true,accepted:[{operation:'scan-slot',stage:'accepted-reviewing',code:2}],publications:[copy(publication),copy(publication)],sources:[{prs:copy(rows),phase:'ready',coverage:'complete',lastReceivedAt:publication.last_received_at},{prs:copy(rows),phase:'ready',coverage:'complete',lastReceivedAt:publication.last_received_at}],native:{graphql:{remaining:97}},inventories:[copy(inv),copy(inv)]}};
 return {e,ledger,p};
}
test('complete replayable natural quota evidence passes',()=>{const {e,ledger,p}=valid();assert.equal(assertQuotaScenario(e,ledger,p).documents,7);});
test('scenario rejects each missing obligation, forged receipts and fault shortcuts',()=>{
 for(const key of ['reserve','foreground','exhaustion','quiescence','retained','recovery','blockedPublications']){const {e,ledger,p}=valid();delete e[key];assert.throws(()=>assertQuotaScenario(e,ledger,p),undefined,key);}
 for(const edit of [e=>e.recovery.autonomous=false,e=>e.blockedPublications[0].prs[0].head_oid='wrong-head',e=>e.recovery.accepted=[],e=>e.foreground.admission.rest.reserveCooldownMs=0,e=>e.recovery.publications[0].receipt_revision=1,e=>e.recovery.sources[0].lastReceivedAt='stale',e=>e.exhaustion.admission.graphql.primaryCooldownMs=0,e=>e.quiescence.restRequests=1,e=>e.quiescence.graphqlRequests=3,e=>e.retained[0].after={authored:[],reviewing:[]},e=>e.recovery.receiptId=5]){const {e,ledger,p}=valid();edit(e);assert.throws(()=>assertQuotaScenario(e,ledger,p));}
 for(const edit of [r=>r.quota={...r.quota,cost:1},r=>r.quota={...r.quota,after:999},r=>r.quota={...r.quota,windowId:'forged'},r=>r.fault='primary']){const {e,ledger,p}=valid();edit(ledger[0]);assert.throws(()=>assertQuotaScenario(e,ledger,p));}
});

test('rejects duplicate or replaced recovered identities for either role and list',()=>{
 for(const role of [0,1])for(const list of ['authored','reviewing'])for(const replacement of ['duplicate','foreign']){const {e,ledger,p}=valid();const ids=e.recovery.inventories[role][list];ids[0]=replacement==='duplicate'?ids[1]:'synthetic-lab/repo-1/999';assert.throws(()=>assertQuotaScenario(e,ledger,p),undefined,`${role}/${list}/${replacement}`);}
});
test('requires complete exact initial and retained fixture inventories',()=>{
 for(const role of [0,1])for(const stage of ['before','after'])for(const list of ['authored','reviewing']){const {e,ledger,p}=valid();e.retained[role][stage][list]=e.retained[role][stage][list].slice(0,1);if(stage==='before'&&list==='reviewing')e.blockedPublications[role].prs=e.blockedPublications[role].prs.filter(r=>`${r.repo}/${r.number}`===e.retained[role].before.reviewing[0]);assert.throws(()=>assertQuotaScenario(e,ledger,p),undefined,`${role}/${stage}/${list}`);}
});
test('rejects duplicate publication rows and corrupted recovered or mounted facts',()=>{
 for(const role of [0,1])for(const witness of ['publications','sources'])for(const field of ['identity','head_oid','base_ref','is_draft']){const {e,ledger,p}=valid();const rows=e.recovery[witness][role].prs;if(field==='identity')rows[0]=structuredClone(rows[1]);else rows[0][field]=field==='is_draft'?true:'corrupted';assert.throws(()=>assertQuotaScenario(e,ledger,p),undefined,`${role}/${witness}/${field}`);}
});
test('legitimate approval facts do not invalidate unchanged identity head base and draft',()=>{
 const {e,ledger,p}=valid();for(const witness of [...e.blockedPublications,...e.recovery.publications,...e.recovery.sources])witness.prs[0].review='approved';assert.equal(assertQuotaScenario(e,ledger,p).documents,7);
});
