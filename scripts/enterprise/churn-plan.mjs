import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {normalizeWorkload} from './workload.mjs';
export const digest=value=>createHash('sha256').update(JSON.stringify(value)).digest('hex');
export function createChurnPlan(input){
 const workload=normalizeWorkload(input),p=workload.populations;
 assert.ok([150,250].includes(p.reviewing));assert.deepEqual({...p,reviewing:150},{authored:50,reviewing:150,repositories:50,members:50,history:250});assert.equal(workload.cadenceSeconds,60);assert.equal(workload.scheduleVersion,'churn-v2','explicit sustained schedule version required');assert.ok(Number.isSafeInteger(workload.seed),'integer seed required');
 let seed=workload.seed>>>0;const random=()=>{seed=(Math.imul(seed,1664525)+1013904223)>>>0;return seed/4294967296;};
 const row=n=>({identity:`synthetic-lab/repo-${(n-1)%50+1}/${n}`,number:n,head:`head-${n}`,authored:n<=50,requested:n>50,draft:false,queued:false,terminal:'open',approved:false});
 const initial=Array.from({length:p.authored+p.reviewing},(_,i)=>row(i+1));
 const numbers=initial.filter(r=>r.number>53).map(r=>r.number);for(let i=numbers.length-1;i>0;i--){const j=Math.floor(random()*(i+1));[numbers[i],numbers[j]]=[numbers[j],numbers[i]];}
 const events=[],working=structuredClone(initial);const add=(kind,n,next={},role=null)=>{let current=working.find(r=>r.number===n);if(!current){current={...row(n),terminal:'absent',requested:false};working.push(current);}const e={revision:events.length+1,nominalOffsetMs:events.length*75000,kind,identity:current.identity,previousHead:current.head,next:{...current,...next},role};events.push(e);Object.assign(current,e.next);};
 add('navigate',51,{},'desktop');add('approve',51,{approved:true},'desktop');add('navigate',52,{},'paired');add('approve',52,{approved:true},'paired');
 add('head',51,{head:`head-51-seed-${workload.seed}`,approved:false});add('request-remove',numbers[0],{requested:false});add('request-add',numbers[0],{requested:true});add('draft',numbers[1],{draft:true});add('ready',numbers[1],{draft:false});add('queue',numbers[2],{queued:true});add('dequeue',numbers[2],{queued:false});add('close',numbers[3],{terminal:'closed'});add('merge',numbers[4],{terminal:'merged'});add('arrival',p.authored+p.reviewing+1,{terminal:'open',requested:true});
 // A second cycle keeps external mutation obligations live throughout the
 // thirty-minute qualification, including the deliberately held detail.
 add('head',53,{head:`head-53-seed-${workload.seed}`});add('request-remove',numbers[5],{requested:false});add('request-add',numbers[5],{requested:true});add('draft',numbers[6],{draft:true});add('ready',numbers[6],{draft:false});add('queue',numbers[7],{queued:true});add('dequeue',numbers[7],{queued:false});add('head',51,{head:`head-51-seed-${workload.seed}-second`,approved:false});add('request-remove',numbers[8],{requested:false});add('request-add',numbers[8],{requested:true});add('draft',numbers[9],{draft:true});add('ready',numbers[9],{draft:false});
 return {schema:1,workload,initial,events,checkpointRevision:7,policy:{cadenceMs:60000,convergenceMs:180000,spacingMs:75000}};
}
export function initialOracle(plan){return {planHash:digest(plan),revision:0,rows:structuredClone(plan.initial),events:structuredClone(plan.events)};}
export function advanceOracle(state,event,receipt){
 assert.equal(event.revision,state.revision+1,'contiguous plan revision');assert.deepEqual(event,state.events[state.revision],'event must match persisted plan');const prior=state.rows.find(r=>r.identity===event.identity);assert.equal(prior?.head??event.next.head,event.previousHead,'previous head');
 if(event.kind==='approve')assert.deepEqual(receipt,{ok:true,identity:event.identity,head:event.previousHead},'successful identity/current-head mutation receipt required');
 const next=structuredClone(state);next.revision=event.revision;const index=next.rows.findIndex(r=>r.identity===event.identity);if(index<0)next.rows.push(structuredClone(event.next));else next.rows[index]=structuredClone(event.next);return next;
}
export function assertChurnFacts(state,actual){
 const wanted=state.rows.filter(r=>r.terminal==='open'&&(r.authored||r.requested));
 const visible=actual;assert.deepEqual(visible.map(r=>r.identity).sort(),wanted.map(r=>r.identity).sort(),'exact independently planned membership');
 for(const row of wanted){const found=actual.find(r=>r.identity===row.identity);assert.equal(found?.list,row.authored?'authored':'reviewing',`${row.identity} actual hook list`);for(const key of ['head','requested','draft','queued','terminal','approved'])assert.equal(found?.[key],row[key],`${row.identity} ${key}`);}
}
export function assertAcceptedRevision(previous,next){assert.ok(next&&Number.isInteger(next.revision)&&Number.isInteger(next.generation),'accepted receipt revision/generation required');if(previous&&previous.generation===next.generation)assert.ok(next.revision>=previous.revision,'accepted revision must not regress');}
export function assertAcceptedFacts(previous,next){
 assert.ok(typeof next.session==='string'&&Number.isInteger(next.revision),'accepted source receipt required');
 if(!previous||previous.session!==next.session)return;
 assert.ok(next.revision>=previous.revision,'accepted source revision regression');
 if(previous.converged&&previous.oracleRevision===next.oracleRevision)assert.equal(next.converged,true,'accepted facts must not regress after convergence at comparable plan revision');
 for(const row of next.facts){const before=previous.facts.find(r=>r.identity===row.identity&&r.head===row.head);if(before?.approved)assert.equal(row.approved,true,'current-head accepted approval must not regress');}
}
export function assertSustainedChurn(evidence){
 assert.equal(evidence.segmentPass,true);assert.equal(evidence.planComplete,true);assert.equal(evidence.sustainedPass,true);assert.equal(evidence.integrationOnly,false,'integration-only cannot qualify sustained churn');assert.deepEqual(evidence.remaining,[]);
 assert.ok(Array.isArray(evidence.activeSegmentsMs)&&evidence.activeSegmentsMs.length>0);for(const value of evidence.activeSegmentsMs)assert.ok(Number.isFinite(value)&&value>=0,'measured active durations required');assert.ok(evidence.activeSegmentsMs.reduce((a,b)=>a+b,0)>=1800000,'at least 30 active minutes excluding restart downtime');
}
// This declared fixture keeps CI successful and mergeability clean. Readiness
// is therefore an abstract obligation over the scheduled facts, not derive().
export function expectedChurnReady(oracle){return oracle.rows.filter(row=>row.terminal==='open'&&row.requested&&!row.draft&&!row.queued&&!row.approved).map(row=>row.identity).sort();}
export function assertChurnReady(oracle,actual){assert.deepEqual([...actual].sort(),expectedChurnReady(oracle),'actual Ready eligibility must match independent plan');}
