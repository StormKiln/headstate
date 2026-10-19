import test from 'node:test';
import assert from 'node:assert/strict';
import {observeUntil,observeRoles} from './churn.mjs';

test('scheduled wait rejects a transient regression even when endpoint recovers',async()=>{
 let now=0;let reads=0;const endpointHealthy=()=>now!==2000;
 await assert.rejects(observeUntil({deadline:75000,now:()=>now,sleep:async ms=>{now+=ms;},observe:async()=>{reads++;assert.ok(endpointHealthy(),'same-head approved fact regressed');}}),/regressed/);
 assert.equal(now,2000);assert.equal(reads,3);now=75000;assert.equal(endpointHealthy(),true);
});
test('scheduled read-only observations preserve deadline and bounded sampling',async()=>{
 let now=365;const waits=[];let reads=0;
 await observeUntil({deadline:75000,now:()=>now,sleep:async ms=>{waits.push(ms);now+=ms;},observe:async()=>{reads++;}});
 assert.equal(now,75000);assert.equal(reads,76);assert.ok(waits.every(ms=>ms>0&&ms<=1000));
});
test('unconverged first role does not suppress second role regression',async()=>{
 const seen=[];
 await assert.rejects(observeRoles(['desktop','paired'],async role=>{seen.push(role);throw Error(role==='desktop'?'not converged':'accepted fact regression');}),/regression/);
 assert.deepEqual(seen,['desktop','paired']);
});
test('scheduled wait rejects transient mounted draft focus loss',async()=>{
 let now=0;
 await assert.rejects(observeUntil({deadline:5000,now:()=>now,sleep:async ms=>{now+=ms;},observe:async()=>{assert.notEqual(now,1000,'draft focus lost');}}),/focus lost/);
});

test('serialized schedule accepts multiple valid slow events without cumulative drift rejection',async()=>{
 const {assertChurnSchedule}=await import('./churn-plan.mjs');const policy={spacingMs:75000,convergenceMs:180000};
 const events=Array.from({length:8},(_,i)=>({revision:i+1,actualOffsetMs:i*120000,convergedOffsetMs:(i+1)*120000}));
 assertChurnSchedule(events,events.map(e=>e.revision),policy);
 for(const bad of [events.slice(1),events.map((e,i)=>i===3?{...e,convergedOffsetMs:e.actualOffsetMs+180001}:e),events.map((e,i)=>i===1?{...e,actualOffsetMs:74999}:e)])assert.throws(()=>assertChurnSchedule(bad,events.map(e=>e.revision),policy));
});

test('healthy policy derives scale and event-kind deadlines',async()=>{
 const {churnPolicy,churnDeadline}=await import('./churn-plan.mjs');const p=churnPolicy(150),large=churnPolicy(250);
 assert.equal(churnDeadline('approve',p),180000);assert.equal(churnDeadline('request-remove',p),600000);assert.equal(churnDeadline('close',p),540000);assert.equal(churnDeadline('draft',p),510000);assert.equal(churnDeadline('request-remove',large),780000);
});
test('native scan phase budget includes admission and publication, not just HTTP',async()=>{
 const {assertHealthyChurnPhases}=await import('./churn-plan.mjs');
 const events=[{operation:'scan-slot',stage:'begin',id:1,ns:0},{operation:'scan-slot',stage:'cancelled',id:1,ns:1000000},{operation:'scan-slot',stage:'accepted-reviewing',id:1,ns:16000000000}];
 assert.throws(()=>assertHealthyChurnPhases(events),/phase.*bounded/);events[2].ns=14000000000;assert.equal(assertHealthyChurnPhases(events).maxMs,14000);
});
test('repeated affected-list requests and timestamps cannot renew stalled progress',async()=>{
 const {observeChurnProgress}=await import('./churn-plan.mjs');let state;const event={identity:'synthetic-lab/repo-5/155',previousHead:'head-155'};const entry={status:200,responseDelivered:true,searches:[{query:'is:pr is:open review-requested:@me',after:null,ids:['PR_155']}]};
 state=observeChurnProgress(state,{event,ledger:[entry],clients:[],now:0});
 state=observeChurnProgress(state,{event,ledger:[entry,{...entry,id:2,at:120000}],clients:[],now:120000});
 assert.throws(()=>observeChurnProgress(state,{event,ledger:[entry,{...entry,id:3,at:180001}],clients:[],now:180001}),/progress.*bounded/);
});

test('native reader retains partial line, consumes new bytes once and rejects truncation',async()=>{
 const {createNativeReader}=await import('./churn.mjs');const {mkdtemp,writeFile,appendFile}=await import('node:fs/promises');const {tmpdir}=await import('node:os');const dir=await mkdtemp(tmpdir()+'/r109-native-reader-'),file=dir+'/native.ndjson';await writeFile(file,'{"seq":1,"ns":0}\n{"seq":2');const read=createNativeReader(file);
 assert.deepEqual(await read(),[{seq:1,ns:0}]);assert.deepEqual(await read(),[]);await appendFile(file,',"ns":1}\n');assert.deepEqual(await read(),[{seq:2,ns:1}]);await writeFile(file,'');await assert.rejects(read(),/truncat/);
});

test('native reader reorders concurrent arrivals and rejects true duplicate',async()=>{
 const {createNativeReader}=await import('./churn.mjs');const {mkdtemp,writeFile,appendFile}=await import('node:fs/promises');const {tmpdir}=await import('node:os');const dir=await mkdtemp(tmpdir()+'/r109-native-order-'),file=dir+'/native.ndjson';
 const prefix=Array.from({length:1737},(_,i)=>JSON.stringify({seq:i+1,ns:i})).join('\n')+'\n';await writeFile(file,prefix+'{"seq":1739,"ns":1740}\n');const read=createNativeReader(file);assert.equal((await read()).length,1737);assert.deepEqual(await read(),[]);await appendFile(file,'{"seq":1738,"ns":1741}\n');assert.deepEqual((await read()).map(e=>e.seq),[1738,1739]);await appendFile(file,'{"seq":1738,"ns":1742}\n');await assert.rejects(read(),/duplicate/);
});
test('native final proof rejects missing sequence, unfinished overbudget and backward slot time',async()=>{
 const {assertHealthyChurnPhases}=await import('./churn-plan.mjs');
 const start={seq:1,operation:'scan-slot',stage:'begin',id:1,ns:100};
 assert.throws(()=>assertHealthyChurnPhases([start,{seq:3,ns:200}]),/contiguous/);
 assert.throws(()=>assertHealthyChurnPhases([start,{seq:2,operation:'scan-slot',stage:'cancelled',id:1,ns:99}]),/monotonic/);
 assert.throws(()=>assertHealthyChurnPhases([start,{seq:2,ns:16000000100}]),/bounded/);
 assert.equal(assertHealthyChurnPhases([start,{seq:2,operation:'scan-slot',stage:'accepted-reviewing',id:1,ns:1000000100},{seq:3,operation:'scan-slot',stage:'cancelled',id:1,ns:2000000100}]).maxMs,2000);
});
test('longer background envelope never admits wrong facts or missing obligations',async()=>{
 const {createChurnPlan,initialOracle,churnPolicy,assertChurnSchedule,assertChurnFacts}=await import('./churn-plan.mjs');const {defaultWorkload}=await import('./workload.mjs');const plan=createChurnPlan({...defaultWorkload,scenario:'churn',scheduleVersion:'churn-v3'}),oracle=initialOracle(plan);
 assertChurnSchedule([{revision:1,kind:'request-remove',actualOffsetMs:0,convergedOffsetMs:400000}],[1],churnPolicy(150));
 assert.throws(()=>assertChurnSchedule([],[1],churnPolicy(150)),/skipped/);assert.throws(()=>assertChurnFacts(oracle,[]),/membership/);
});

for(const timestamps of [false,true])test(`wrong retained facts do not advance with timestamps=${timestamps}`,async()=>{
 const {observeChurnProgress}=await import('./churn-plan.mjs');const event={identity:'synthetic-lab/repo-5/155',previousHead:'head-155'};let state;
 const sample=now=>({event,ledger:[],now,clients:[{accepted:{receipt_revision:now,last_received_at:String(now)},facts:[{identity:event.identity,head:event.previousHead,requested:true,draft:false,queued:false,terminal:'open',approved:false,observation:{state:'retained',last_observed_at:timestamps?String(now):'old',unknown_fields:[],retained_fields:[],confirmed_review:{head_oid:event.previousHead,review:'none',confirmed_at:timestamps?String(now):'old',receipt:{id:now}}}}]}]});
 state=observeChurnProgress(state,sample(0));state=observeChurnProgress(state,sample(120000));assert.throws(()=>observeChurnProgress(state,sample(180001)),/progress.*bounded/);
});
test('real semantic and unknown-to-known progression count once, alternating seen values do not',async()=>{
 const {observeChurnProgress}=await import('./churn-plan.mjs');const event={identity:'synthetic-lab/repo-5/155',previousHead:'old'};let state;
 const sample=(now,head,unknown)=>({event,ledger:[],now,clients:[{accepted:{receipt_revision:now},facts:[{identity:event.identity,head,requested:true,observation:{state:'retained',unknown_fields:unknown}}]}]});
 state=observeChurnProgress(state,sample(0,'old',['head']));state=observeChurnProgress(state,sample(120000,'new',['head']));assert.equal(state.lastAt,120000);state=observeChurnProgress(state,sample(240000,'new',[]));assert.equal(state.lastAt,240000);state=observeChurnProgress(state,sample(360000,'old',['head']));assert.equal(state.lastAt,240000);assert.throws(()=>observeChurnProgress(state,sample(420001,'new',[])),/progress.*bounded/);
});
