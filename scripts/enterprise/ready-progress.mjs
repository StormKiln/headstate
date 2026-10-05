import assert from 'node:assert/strict';
import {writeFile} from 'node:fs/promises';
import {resolve} from 'node:path';
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms));
/** Focused mounted production hooks + real native commands/admission. No mocked
 * row answers, artificial native clock, allowance reset, or background poll. */
export async function runReadyProgress({page,nativeCall,provider,result,out}) {
 await page.getByTestId('ready-progress-loaded').filter({hasText:/^16$/}).waitFor({timeout:60000});
 const evidence={scenario:'zero-dispatch debt',visibleNumbers:[51,52,53,54,55,56],populationNumbers:Array.from({length:16},(_,i)=>51+i),clock:'real30s',warmup:[],snapshots:[]};
 try {
  const before=provider.ledger.length;
  for(const number of [151,152,153,154]) {
   const row=provider.data.rows.find(row=>row.number===number);
   const response=await nativeCall('desktop','get_ready_stacks',{rows:[{source:{provider:'github',host:'github.com'},repo:row.repository.nameWithOwner,number,head_oid:row.headRefOid,base_ref:row.baseRefName}]});
   assert.equal(response.status,200);const body=await response.json();assert.equal(body.value[0].stack.kind,'none');
   evidence.warmup.push({number,callId:body.callId});
  }
  assert.equal(provider.ledger.length-before,8,'four real two-document stack lookups spend the actual native allowance');
  const mountedAt=await page.evaluate(()=>performance.now());
  await page.getByRole('button',{name:'Mount Ready hooks',exact:true}).click();
  await page.waitForFunction(start=>window.__enterprise.telemetry.filter(e=>e.kind==='call'&&e.name.startsWith('get_ready_')&&e.at>=start).length>=16,mountedAt,{timeout:15000});
  const initial=await page.evaluate(start=>window.__enterprise.telemetry.filter(e=>e.kind==='call'&&e.name.startsWith('get_ready_')&&e.at>=start),mountedAt);
  const tail=Object.fromEntries(['get_ready_pushers','get_ready_stacks'].map(command=>[command,[...new Set(initial.filter(e=>e.name===command).flatMap(e=>e.rows.map(r=>r.number)).filter(n=>n>56))].sort((a,b)=>a-b)]));
  for(const numbers of Object.values(tail))assert.equal(numbers.length,2,'six visible and two tail identities were actually invoked');
  const initialProvider=provider.ledger.filter(e=>e.id>before+8&&['stack-down','stack-up','pusher-activity','rules','protection'].includes(e.stage));
  assert.equal(initialProvider.length,0,'initial mounted advisory calls genuinely receive zero provider attempts');
  evidence.initialTail=tail;evidence.snapshots.push({stage:'initial-refused',at:performance.now(),commands:initial,queries:await page.evaluate(()=>window.__enterprise.querySummary())});
  // The two clocks run unmodified. Wait for the next production window and
  // its finite drain; never invalidate/refetch manually to manufacture progress.
  await delay(32000);
  const later=await page.evaluate(start=>window.__enterprise.telemetry.filter(e=>e.kind==='call'&&e.name.startsWith('get_ready_')&&e.at>=start+29000),mountedAt);
  evidence.snapshots.push({stage:'next-window',at:performance.now(),commands:later,queries:await page.evaluate(()=>window.__enterprise.querySummary())});
  const failures=[];
  for(const command of ['get_ready_pushers','get_ready_stacks']) {
   const useful=later.filter(e=>e.name===command).flatMap(e=>e.reply??[]).filter(row=>command==='get_ready_pushers'?row.last_known_pusher:row.last_known_stack).map(row=>row.number);
   if(!tail[command].some(number=>useful.includes(number)))failures.push(`${command}: deferred tail lost its opportunity; initial=${tail[command]}, useful=${useful}`);
   if(!evidence.visibleNumbers.some(number=>useful.includes(number)))failures.push(`${command}: continuously pending visible demand received no useful evidence; useful=${useful}`);
  }
  evidence.failures=failures;assert.deepEqual(failures,[],'both actual visible and deferred tail work must make useful progress');
  result.phases.push({name:'mounted-deferred-tail-keeps-opportunity',applied:true,exercised:true,converged:true});
 } finally {await writeFile(resolve(out,'ready-progress.json'),JSON.stringify(evidence,null,2));}
}

/** Two real authenticated roles have separate QueryClients, disjoint repository
 * identities, and the SAME native allowance. This is a qualification probe;
 * it does not reset the native meter or manually refetch either consumer. */
export async function runDualReadyProgress({pages,provider,result,out,leader,control,nativeCall,lifecycle=false}) {
 const roles=['desktop','paired'];const first=roles.indexOf(leader),second=1-first;
 const evidence={scenario:'two-disjoint-ready-consumers',leader,mountStaggerMs:2000,graphqlDeliveryDelayMs:40,cycles:4,cycleMs:30000,deadlineMs:140000,populations:{desktop:[51,66],paired:[76,91]},snapshots:[]};
 try {
  for(const page of pages)await page.getByTestId('ready-progress-loaded').filter({hasText:/^16$/}).waitFor({timeout:60000});
  provider.fault.delay=40;
  const begin=performance.now();
  await pages[first].getByRole('button',{name:'Mount Ready hooks',exact:true}).click();
  await delay(2000);
  await pages[second].getByRole('button',{name:'Mount Ready hooks',exact:true}).click();
  for(let cycle=0;cycle<=4;cycle++) {
   if(cycle)await delay(30000);else await delay(1500);
   const rolesSnapshot=[];
   for(const [i,page] of pages.entries())rolesSnapshot.push({role:roles[i],commands:await page.evaluate(()=>window.__enterprise.telemetry.filter(e=>e.kind==='call'&&e.name.startsWith('get_ready_'))),queries:await page.evaluate(()=>window.__enterprise.querySummary().filter(q=>['ready-pushers','ready-stack'].includes(q.kind)))});
   evidence.snapshots.push({cycle,elapsedMs:performance.now()-begin,roles:rolesSnapshot,providerReceipts:provider.ledger.length});
   await writeFile(resolve(out,'ready-dual-progress.json'),JSON.stringify(evidence,null,2));
  }
  const failures=[];
  for(const snapshot of evidence.snapshots.at(-1).roles) {
   const minimum=snapshot.role==='desktop'?51:76;
   for(const command of ['get_ready_pushers','get_ready_stacks']) {
    const replies=snapshot.commands.filter(e=>e.name===command).flatMap(e=>e.reply??[]);
    const admitted=replies.filter(r=>r.advisory_progress?.admitted);
    const measured=replies.filter(r=>command==='get_ready_pushers'?r.last_known_pusher:r.last_known_stack);
    if(!admitted.length)failures.push(`${snapshot.role}/${command}: no actual admitted command in four real cycles`);
    if(!measured.some(r=>r.number<minimum+6))failures.push(`${snapshot.role}/${command}: visible demand never measured`);
    if(!measured.some(r=>r.number>=minimum+6))failures.push(`${snapshot.role}/${command}: tail demand never measured`);
   }
  }
  evidence.failures=failures;assert.deepEqual(failures,[],'both disjoint consumers must receive measured opportunities in this bounded qualification');
  result.phases.push({name:'two-disjoint-ready-consumers',applied:true,exercised:true,converged:true});
  if(lifecycle) {
   const paired=pages[1];
   await paired.getByRole('button',{name:'Unmount Ready hooks',exact:true}).click();
   await delay(1500);
   const count=await paired.evaluate(()=>window.__enterprise.telemetry.filter(e=>e.kind==='call'&&e.name.startsWith('get_ready_')).length);
   await delay(31000);
   assert.equal(await paired.evaluate(()=>window.__enterprise.telemetry.filter(e=>e.kind==='call'&&e.name.startsWith('get_ready_')).length),count,'unmounted Ready demand stops across a real window');
   assert.ok((await paired.evaluate(()=>window.__enterprise.querySummary().filter(q=>['ready-pushers','ready-stack'].includes(q.kind)))).every(q=>q.observers===0),'no mounted advisory observers remain');
   await control('revoke');
   const refused=await nativeCall('paired','get_viewer');assert.ok(!refused.ok,'retired paired transport is immediately refused');
   await control('repair');
   assert.equal((await nativeCall('paired','get_viewer')).status,200,'new authenticated incarnation works');
   const start=await paired.evaluate(()=>performance.now());
   await paired.getByRole('button',{name:'Mount Ready hooks',exact:true}).click();
   await paired.waitForFunction(start=>window.__enterprise.telemetry.some(e=>e.kind==='call'&&e.name.startsWith('get_ready_')&&e.at>=start&&(e.reply??[]).some(r=>r.advisory_progress?.admitted&&(r.last_known_pusher||r.last_known_stack))),start,{timeout:65000});
   evidence.lifecycle={unmountedWindowMs:31000,retiredTransportStatus:refused.status,repairedAutomaticCommands:await paired.evaluate(start=>window.__enterprise.telemetry.filter(e=>e.kind==='call'&&e.name.startsWith('get_ready_')&&e.at>=start),start)};
   result.phases.push({name:'ready-unmount-revoke-repair-production-reacquisition',applied:true,exercised:true,converged:true});
  }

 }finally {await writeFile(resolve(out,'ready-dual-progress.json'),JSON.stringify(evidence,null,2));}
}
