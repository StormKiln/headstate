import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {resolve} from 'node:path';
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms));
const advisory=entry=>['stack-down','stack-up','pusher-activity','rules','protection'].includes(entry.stage);
export function advisoryPeriods(events){
 const periods=[];
 for(const event of events){
  if(event.operation==='advisory-cycle'&&event.stage==='reset')periods.push({atNs:event.ns,allocations:{},debits:{},demands:[]});
  const period=periods.at(-1);if(!period||event.operation!=='advisory-share')continue;
  if(event.stage==='allocated')period.allocations[event.id]=event.code;
  if(event.stage==='debit')period.debits[event.id]=(period.debits[event.id]??0)+event.code;
  if(event.stage==='demand')period.demands.push({principal:event.id,ns:event.ns});
 }
 return periods;
}
export async function runReadyCompletion({pages,provider,result,out,control,nativeCall}){
 const began=performance.now(),ceilingMs=900000,expected=Array.from({length:16},(_,i)=>51+i);
 const evidence={ceilingMs,clock:'real30s selection / real60s Stats',layout:'desktop production ReadyStrip16; paired production Stats after brief disjoint Ready interest',expected,setup:[],snapshots:[],periods:[]};
 const save=()=>writeFile(resolve(out,'ready-completion.json'),JSON.stringify(evidence,null,2));
 const events=async()=>{const text=await readFile(resolve(out,'profile/native.ndjson'),'utf8');return text.slice(0,text.lastIndexOf('\n')).split('\n').filter(Boolean).map(JSON.parse);};
 const row=n=>provider.data.rows.find(row=>row.number===n);
 const call=async(command,number)=>{
  const r=row(number);const ask=command==='get_ready_stacks'?{source:{provider:'github',host:'github.com'},repo:r.repository.nameWithOwner,number,head_oid:r.headRefOid,base_ref:r.baseRefName}:{repo:r.repository.nameWithOwner,number,base:r.baseRefName,head_repo:r.headRepository.nameWithOwner,head_ref:r.headRefName,head_oid:r.headRefOid};
  const before=provider.ledger.length,response=await nativeCall('desktop',command,{rows:[ask]});const body=await response.json();
  evidence.setup.push({command,number,status:response.status,callId:body.callId,reply:body.value,provider:provider.ledger.slice(before).filter(advisory),elapsedMs:performance.now()-began});await save();assert.equal(response.status,200);return body.value[0];
 };
 try{
  await control('start');
  for(const page of pages)await page.getByTestId('counts').filter({hasText:'Authored 50 / Reviewing 150'}).waitFor({timeout:120000});
  // Real setup traffic: policy+activity2, shared-policy distinct-head activity1,
  // two standalone stacks4, then the eighth attempt stores only target's down leg.
  const before=provider.ledger.filter(advisory).length;
  await call('get_ready_pushers',151);await call('get_ready_pushers',101);
  await call('get_ready_stacks',152);await call('get_ready_stacks',153);
  assert.equal(provider.ledger.filter(advisory).length-before,7);
  const partial=await call('get_ready_stacks',51);
  assert.equal(partial.advisory_progress?.outcome,'partial');assert.equal(partial.stack.kind,'unknown');
  const firstDown=provider.ledger.find(e=>e.stage==='stack-down'&&e.subjects?.some(s=>s.number===51));
  assert.ok(firstDown);
  await pages[1].getByRole('button',{name:'To Review',exact:true}).click();
  await pages[1].waitForFunction(()=>window.__enterprise.telemetry.some(e=>e.kind==='call'&&e.name.startsWith('get_ready_')));
  await pages[1].getByRole('button',{name:'Statistics',exact:true}).click();
  await delay(65000);
  evidence.controlledSetupMs=performance.now()-began;
  await pages[0].getByRole('button',{name:'To Review',exact:true}).click();
  const mounted=performance.now();let lastMoved=-1;
  while(performance.now()-began<ceilingMs){
   await delay(15000);
   const minute=Math.floor((performance.now()-mounted)/60000);
   if(minute!==lastMoved){const rows=pages[0].locator('[data-advisory-key]');await rows.nth((minute*8)%16).scrollIntoViewIfNeeded();lastMoved=minute;}
   const ui=await pages[0].evaluate(()=>({queries:window.__enterprise.querySummary(),commands:window.__enterprise.telemetry.filter(e=>e.kind==='call'&&e.name.startsWith('get_ready_')),identities:[...document.querySelectorAll('[data-advisory-key]')].map(n=>Number(JSON.parse(n.getAttribute('data-advisory-key'))[3])).sort((a,b)=>a-b)}));
   const measured=(kind,field)=>[...new Set(ui.queries.filter(q=>q.kind===kind&&expected.includes(q.syntheticNumber)&&q[field]).map(q=>q.syntheticNumber))].sort((a,b)=>a-b);
   const snapshot={elapsedMs:performance.now()-began,mountedMs:performance.now()-mounted,minute,ui,pushers:measured('ready-pushers','measuredPusher'),rules:measured('ready-pushers','measuredRules'),stacks:measured('ready-stack','measuredStack'),providerReceipts:provider.ledger.length};
   evidence.snapshots.push(snapshot);evidence.periods=advisoryPeriods(await events());await save();
   console.log(JSON.stringify({readyCompletionMs:snapshot.elapsedMs,pushers:snapshot.pushers.length,rules:snapshot.rules.length,stacks:snapshot.stacks.length}));
   assert.deepEqual(ui.identities,expected,'exact mounted16 production rows');
   if([snapshot.pushers,snapshot.rules,snapshot.stacks].every(ids=>ids.length===16))break;
  }
  const last=evidence.snapshots.at(-1);
  const finalEvents=await events();evidence.periods=advisoryPeriods(finalEvents);
  const lastPaired=Math.max(...finalEvents.filter(e=>e.operation==='advisory-share'&&e.stage==='demand'&&e.id!==0).map(e=>e.ns));
  const solo=evidence.periods.find(p=>p.atNs>lastPaired&&p.allocations[0]===8&&Object.keys(p.allocations).length===1&&p.debits[0]===8);
  const target=provider.ledger.filter(e=>['stack-down','stack-up'].includes(e.stage)&&e.subjects?.some(s=>s.number===51));
  evidence.expiredStage={target,firstDownAt:firstDown.at,subsequentDown:target.find(e=>e.stage==='stack-down'&&e.at-firstDown.at>=60000)};
  evidence.lastPairedDemandNs=lastPaired;evidence.soloEight=solo;evidence.elapsedMs=performance.now()-began;await save();
  for(const ids of [last?.pushers,last?.rules,last?.stacks])assert.deepEqual(ids,expected,'all16 useful identities finish under declared900s inclusive ceiling');
  assert.ok(Number.isFinite(lastPaired)&&solo,'paired interest expires and desktop actually debits its solo8');
  assert.ok(evidence.expiredStage.subsequentDown&&target.some(e=>e.stage==='stack-up'&&e.at>=evidence.expiredStage.subsequentDown.at),'expired first leg is fetched again before useful completion');
  for(const period of evidence.periods){assert.ok(Object.values(period.debits).reduce((a,b)=>a+b,0)<=8);for(const [principal,count]of Object.entries(period.debits))assert.ok(count<=period.allocations[principal]);}
  result.phases.push({name:'one-ready-moving-viewport-all16-useful-after-expired-stage',applied:true,exercised:true,converged:true});
  result.readyCompletion={elapsedMs:evidence.elapsedMs,controlledSetupMs:evidence.controlledSetupMs,ceilingMs,lastPairedDemandNs:lastPaired,soloEightAtNs:solo.atNs};
 }finally{await save();}
}
