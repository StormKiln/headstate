import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
// Synthetic workload weights, never a prediction of GitHub pricing.
export const weightedQuotaProfile={id:'weighted-natural-v1',graphql:{capacity:12000,windowSeconds:150},rest:{capacity:800,windowSeconds:150},burst:{windowSeconds:1,maxCost:10000,cooldownSeconds:2},costs:{base:1,field:1,page:4,detail:30,advisory:6,mutation:80,restActivity:80,restRules:40,restProtection:20,restWrite:80}};
export function validateQuotaProfile(input){
 assert.ok(input&&typeof input.id==='string'&&input.id.length,'quota profile id required');
 const p=structuredClone(input);
 assert.deepEqual(Object.keys(p).sort(),['burst','costs','graphql','id','rest']);
 for(const bucket of ['graphql','rest']){assert.deepEqual(Object.keys(p[bucket]).sort(),['capacity','windowSeconds']);for(const n of Object.values(p[bucket]))assert.ok(Number.isSafeInteger(n)&&n>0,'positive integer quota required');}
 assert.deepEqual(Object.keys(p.burst).sort(),['cooldownSeconds','maxCost','windowSeconds']);
 assert.deepEqual(Object.keys(p.costs).sort(),['advisory','base','detail','field','mutation','page','restActivity','restProtection','restRules','restWrite'].sort());
 for(const n of [...Object.values(p.burst),...Object.values(p.costs)])assert.ok(Number.isSafeInteger(n)&&n>0,'positive integer quota weight required');
 return p;
}
export function quotaIdentity(input){const p=validateQuotaProfile(input);const canonical=o=>o&&typeof o==='object'?Object.fromEntries(Object.keys(o).sort().map(k=>[k,canonical(o[k])])):o;return {id:p.id,hash:'sha256:'+createHash('sha256').update(JSON.stringify(canonical(p))).digest('hex')};}
export function documentCost(profile,{doc,method='GET',path=''}){
 const c=profile.costs;
 if(!doc)return method!=='GET'?c.restWrite:new URL(path,'http://synthetic.invalid').pathname.endsWith('/activity')?c.restActivity:path.endsWith('/protection')?c.restProtection:c.restRules;
 const walk=fields=>fields.reduce((sum,f)=>sum+(f.name==='rateLimit'?0:c.field+(f.args.first?Math.ceil(f.args.first/25)*c.page:0)+(['body','reviewThreads','latestReviews'].includes(f.name)?c.detail:0)+(['pullRequests','ref'].includes(f.name)?c.advisory:0)+walk(f.selection)),0);
 return c.base+walk(doc.selection)+(doc.kind==='mutation'?c.mutation:0);
}
export function createQuotaState(input,origin=Date.now()){
 const profile=validateQuotaProfile(input),states={};let burst=[],blockedUntil=0,last=origin;
 for(const bucket of ['graphql','rest'])states[bucket]={index:0,remaining:profile[bucket].capacity};
 return {charge({bucket,cost,now=Date.now()}){
  assert.ok(Object.hasOwn(states,bucket));assert.ok(Number.isSafeInteger(cost)&&cost>0);assert.ok(Number.isFinite(now)&&now>=last,'request clock must be monotonic');last=now;
  const policy=profile[bucket],state=states[bucket],index=Math.floor((now-origin)/(policy.windowSeconds*1000));
  if(index!==state.index){state.index=index;state.remaining=policy.capacity;}
  const before=state.remaining,reset=Math.ceil((origin+(index+1)*policy.windowSeconds*1000)/1000);
  burst=burst.filter(r=>r.at>now-profile.burst.windowSeconds*1000);
  const naturalLimit=cost>before;
  if(now>=blockedUntil&&burst.reduce((sum,r)=>sum+r.cost,0)+cost>profile.burst.maxCost)blockedUntil=now+profile.burst.cooldownSeconds*1000;
  const secondaryLimit=now<blockedUntil,charged=naturalLimit||secondaryLimit?0:cost;
  state.remaining-=charged;if(charged)burst.push({at:now,cost:charged});
  return Object.freeze({origin,receivedAt:now,bucket,windowId:`${bucket}:${origin}:${index}`,capacity:policy.capacity,before,cost,charged,after:state.remaining,reset,naturalLimit,secondaryLimit,retryAfter:secondaryLimit?Math.ceil((blockedUntil-now)/1000):0});
 }};
}
export function reconcileQuota(profile,receipts){
 assert.ok(receipts.length,'quota receipts required');const state=createQuotaState(profile,receipts[0].origin),charged={rest:0,graphql:0};
 for(const row of receipts){const expected=state.charge({bucket:row.bucket,cost:row.cost,now:row.receivedAt});assert.deepEqual(row,expected,'quota receipt must replay exactly');charged[row.bucket]+=row.charged;}
 return {documents:receipts.length,charged};
}
