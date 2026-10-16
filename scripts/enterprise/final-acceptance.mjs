import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import {resolve} from 'node:path';
import {advisoryPeriods,usefulPublications} from './ready-completion.mjs';
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms));
export const soakPolicy={minimumMs:1800000,interestMarginMs:180000,usefulBlockMs:120000,finalMarginMs:300000,flatProgressMs:600000,checkpointMs:60000,meaning:'conditional synthetic diagnostic ceiling, not a universal liveness guarantee or SLA'};
export function remainingWork(coverage,expected){return {pushers:expected.filter(n=>!coverage.pushers.includes(n)||!coverage.rules.includes(n)),stacks:expected.filter(n=>!coverage.stacks.includes(n))};}
export function diagnosticCeiling(remaining){return Math.max(soakPolicy.minimumMs,soakPolicy.interestMarginMs+soakPolicy.usefulBlockMs*Math.max(remaining.pushers.length,remaining.stacks.length)+soakPolicy.finalMarginMs);}
export function assertAdvisoryPeriods(periods){for(const period of periods){assert.ok(Object.values(period.debits).reduce((a,b)=>a+b,0)<=8,'actual period advisory debits stay within8');for(const [principal,count]of Object.entries(period.debits))assert.ok(count<=period.allocations[principal],'actual principal debits stay within allocated share');}}
export function displayEvidence(queries,expected){
 const population=new Set(expected);
 return Object.fromEntries([['pushers','ready-pushers','pusher','measuredPusher'],['rules','ready-pushers','rules','measuredRules'],['stacks','ready-stack','stack','measuredStack']].map(([family,kind,evidence,retained])=>{
  const matching=queries.filter(q=>q.kind===kind&&population.has(q.syntheticNumber));
  const fresh=[...new Set(matching.filter(q=>q.evidence?.[evidence]?.fresh).map(q=>q.syntheticNumber))].sort((a,b)=>a-b);
  const held=[...new Set(matching.filter(q=>q[retained]).map(q=>q.syntheticNumber))].sort((a,b)=>a-b);
  return [family,{fresh,retained:held,unresolved:expected.filter(n=>!held.includes(n))}];
 }));
}
export async function mountedReadyIdentities(page){return page.locator('[data-advisory-key]').evaluateAll(nodes=>nodes.map(n=>{const key=JSON.parse(n.getAttribute('data-advisory-key'));return `${key[2]}/${key[3]}`;}).sort());}
export function verifiedApprovedIdentities(rows,ledger){
 const approved=rows.filter(r=>r.reviews.nodes.some(v=>v.author.login==='synthetic-viewer'&&v.state==='APPROVED'&&v.commit.oid===r.headRefOid));
 assert.deepEqual(approved.map(r=>r.number).sort((a,b)=>a-b),[51,52,53],'exact three successful current-head approvals');
 const writes=ledger.filter(e=>e.operation==='review-write');assert.equal(writes.length,3);assert.ok(writes.every(e=>e.status===200&&e.terminal==='response-sent'),'all three writes succeeded at the provider');
 return approved.map(r=>`${r.repository.nameWithOwner}/${r.number}`).sort();
}
export async function runEnterpriseSoak({pages,provider,result,out,profile}){
 const sourceNumbers=Array.from({length:148},(_,i)=>51+i),open=provider.data.rows.filter(r=>r.state==='OPEN');
 const authored=open.filter(r=>r.author.login==='synthetic-viewer').map(r=>`${r.repository.nameWithOwner}/${r.number}`).sort();
 const approved=verifiedApprovedIdentities(provider.data.rows,provider.ledger);
 const reviewing=open.filter(r=>sourceNumbers.includes(r.number)).map(r=>`${r.repository.nameWithOwner}/${r.number}`).sort();
 const eligible=reviewing.filter(id=>!approved.includes(id));assert.equal(eligible.length,145);
 const eligibleNumbers=open.filter(r=>eligible.includes(`${r.repository.nameWithOwner}/${r.number}`)).map(r=>r.number).sort((a,b)=>a-b);
 const expectedNumbers=eligibleNumbers; // Fixed continuing demand, independently derived before the loop.
 const readNative=async()=>{const text=await readFile(resolve(profile,'native.ndjson'),'utf8');return text.slice(0,text.lastIndexOf('\n')).split('\n').filter(Boolean).map(JSON.parse);};
 const snapshot=page=>page.evaluate(()=>({inventory:window.__enterprise.inventory(),readyEligibility:window.__enterprise.readyEligibility(),queries:window.__enterprise.querySummary(),publications:window.__enterprise.advisoryPublications,counts:document.querySelector('[data-testid="counts"]')?.textContent,lost:window.__enterprise.measurement.lost}));
 await pages[0].getByRole('button',{name:'Back to list',exact:true}).click();
 const initial=await snapshot(pages[0]);
 const initialCoverage=usefulPublications(initial.publications,expectedNumbers),remaining=remainingWork(initialCoverage,expectedNumbers),ceilingMs=diagnosticCeiling(remaining);
 const start=performance.now();result.soak={...soakPolicy,ceilingMs,initialRemaining:remaining,initialCoverage,expectedNumbers,approved,expectedEligible:eligible,expectedReviewing:reviewing,checkpoints:[],obligations:{externalClose:false,advisoryObserved:false,reviewGuidanceObserved:false}};
 const save=()=>writeFile(resolve(out,'soak-progress.json'),JSON.stringify(result.soak,null,2));
 await writeFile(resolve(out,'soak-policy.json'),JSON.stringify({source:JSON.parse(await readFile(resolve(out,'metadata.json'),'utf8')),policy:soakPolicy,remaining,ceilingMs,assumptions:['fixed145 eligible demand = exact148 source minus verified current-head approvals51/52/53','no claim of pre-approval advisory coverage for retired approved rows','one desktop ReadyStrip; paired retained detail','unchanged finite heads/bases and healthy bounded synthetic responses','same one-minute moving viewport and actual30s windows','<=2 documents per cold advisory command; unchanged8-attempt allowance/TTLs','120s per-new-identity planning block qualified by16-row run, not a proof','10min flat useful progress with continuing admission is an earlier diagnostic FAIL']},null,2));
 await save();
 provider.fault.closed=[200];provider.fault.merged=[199];
 let lastNew={pushers:0,stacks:0},lastSize={pushers:remaining.pushers.length,stacks:remaining.stacks.length},lastDebit={pushers:0,stacks:0},lastCheckpoint=-1;
 try{
  while(performance.now()-start<ceilingMs){
   await delay(Math.min(5000,Math.max(0,ceilingMs-(performance.now()-start))));
   const minute=Math.floor((performance.now()-start)/soakPolicy.checkpointMs);if(minute===lastCheckpoint)continue;lastCheckpoint=minute;
   const rows=pages[0].locator('[data-advisory-key]'),count=await rows.count();if(count)await rows.nth((minute*8)%count).scrollIntoViewIfNeeded();
   const ui=await snapshot(pages[0]),paired=await snapshot(pages[1]);
   const native=await readNative(),debits=native.filter(e=>e.operation==='advisory-share'&&e.stage==='debit'),lastNs=native.at(-1)?.ns??0,recentDebit=debits.at(-1)?.ns??0;
   const coverage=usefulPublications(ui.publications,expectedNumbers),owed=remainingWork(coverage,expectedNumbers);
   const db=JSON.parse(execFileSync('python3',['scripts/enterprise/inspect-profile.py',profile],{encoding:'utf8'}));
   const displayed=await mountedReadyIdentities(pages[0]);
   const exact=(a,b)=>JSON.stringify(a)===JSON.stringify(b);
   const sourceClosed=exact(ui.inventory.authored,authored)&&exact(paired.inventory.authored,authored)&&exact(ui.inventory.reviewing,reviewing)&&exact(paired.inventory.reviewing,reviewing);
   result.soak.obligations.externalClose=sourceClosed&&exact(ui.readyEligibility,eligible)&&exact(paired.readyEligibility,eligible)&&exact(displayed,eligible);
   result.soak.obligations.advisoryObserved=owed.pushers.length===0;result.soak.obligations.reviewGuidanceObserved=owed.stacks.length===0;
   result.soak.externalTransitions=Object.fromEntries([199,200].map(number=>{const row=provider.data.rows.find(row=>row.number===number);return [number,{state:row.state,closedAt:row.closedAt,mergedAt:row.mergedAt}];}));
   const elapsedMs=performance.now()-start;
   const checkpoint={elapsedMs,owed,coverage,displayEvidence:displayEvidence(ui.queries,eligibleNumbers),inventory:{desktop:ui.inventory,paired:paired.inventory,displayed,desktopEligible:ui.readyEligibility,pairedEligible:paired.readyEligibility},queries:ui.queries,db,providerReceipts:provider.ledger.length,advisoryDebits:debits.length,nativePeriods:advisoryPeriods(native)};
   result.soak.checkpoints.push(checkpoint);await save();
   if(sourceClosed){assert.deepEqual(ui.readyEligibility,eligible,'production selector matches fixed approval-derived eligibility');assert.deepEqual(paired.readyEligibility,eligible);assert.deepEqual(displayed,eligible,'actual Ready DOM has exactly145 eligible rows, not148 source rows');}
   assertAdvisoryPeriods(checkpoint.nativePeriods);
   assert.equal(ui.lost||paired.lost,false);assert.equal(db.integrity,'ok');
   for(const family of ['pushers','stacks']){
    if(owed[family].length<lastSize[family]){lastNew[family]=elapsedMs;lastSize[family]=owed[family].length;lastDebit[family]=debits.length;}
    if(owed[family].length&&elapsedMs>soakPolicy.interestMarginMs&&elapsedMs-Math.max(lastNew[family],soakPolicy.interestMarginMs)>=soakPolicy.flatProgressMs&&debits.length-lastDebit[family]>=8&&lastNs-recentDebit<120e9){
     result.soak.flatProgressFailure={family,owed:owed[family],lastNewMs:lastNew[family],elapsedMs,admissionContinues:true};await save();assert.fail('10 minutes without a new useful unchanged eligible identity despite continuing admission');
    }
   }
   console.log(JSON.stringify({soakElapsedMs:elapsedMs,ceilingMs,pushersRemaining:owed.pushers.length,stacksRemaining:owed.stacks.length}));
   if(Object.values(result.soak.obligations).every(Boolean)&&result.soak.firstFullConvergenceMs===undefined)result.soak.firstFullConvergenceMs=elapsedMs;
   await save();if(elapsedMs>=soakPolicy.minimumMs&&Object.values(result.soak.obligations).every(Boolean))break;
  }
  result.soak.elapsedMs=performance.now()-start;await save();
  assert.ok(result.soak.elapsedMs>=soakPolicy.minimumMs&&result.soak.elapsedMs<=ceilingMs);assert.ok(Object.values(result.soak.obligations).every(Boolean),'all exact population/DOM/useful obligations before immutable diagnostic ceiling');
  await pages[1].getByText('Synthetic description 52',{exact:false}).waitFor();await delay(3000);
  const detailCalls=()=>pages[1].evaluate(()=>window.__enterprise.telemetry.filter(e=>e.kind==='call'&&e.name==='get_pr_detail').length);
  const countBefore=await detailCalls();await delay(125000);assert.equal(await detailCalls(),countBefore);result.soak.unchangedDetailWindowMs=125000;
  await pages[1].getByRole('button',{name:'Back to list',exact:true}).click();assert.deepEqual(await mountedReadyIdentities(pages[1]),eligible,'actual paired final Ready eligible inventory');
  await save();
 }finally{result.soak.totalWithFinalPhaseMs=performance.now()-start;await save();}
}

// Separate current-session tail qualification, never a continuation of a failed soak.
export async function runEnterpriseTail({pages,provider,result}){
 result.tail={scope:`new native/browser session; ${result.warm?'copied synthetic warm':'fresh synthetic'} profile; not a soak continuation`,startedAt:new Date().toISOString()};
 await pages[0].getByRole('button',{name:'Back to list',exact:true}).click();
 await pages[0].getByRole('button',{name:/Synthetic review 53(?:\D|$)/}).first().click();
 await pages[0].getByText('Synthetic description 53',{exact:false}).waitFor();
 await pages[0].getByRole('button',{name:'Approve',exact:true}).first().click();
 await pages[0].getByRole('button',{name:'Approved',exact:true}).first().waitFor();
 const approved=verifiedApprovedIdentities(provider.data.rows,provider.ledger);result.tail.approved=approved;
 provider.fault.closed=[200];provider.fault.merged=[199];
 for(const page of pages)await page.waitForFunction(()=>window.__enterprise.inventory().reviewing.length===148,null,{timeout:120000});
 const source=Array.from({length:148},(_,i)=>{const n=i+51;return `synthetic-lab/repo-${(n-1)%50+1}/${n}`;}).sort();
 const eligible=source.filter(id=>!approved.includes(id));assert.equal(eligible.length,145);
 for(const page of pages){const ui=await page.evaluate(()=>({inventory:window.__enterprise.inventory(),eligible:window.__enterprise.readyEligibility()}));assert.deepEqual(ui.inventory.reviewing,source);assert.deepEqual(ui.eligible,eligible);}
 await pages[0].getByRole('button',{name:'Back to list',exact:true}).click();assert.deepEqual(await mountedReadyIdentities(pages[0]),eligible);result.tail.desktopDOM=await mountedReadyIdentities(pages[0]);
 await pages[1].getByText('Synthetic description 52',{exact:false}).waitFor();await delay(3000);
 const detailCalls=()=>pages[1].evaluate(()=>window.__enterprise.telemetry.filter(e=>e.kind==='call'&&e.name==='get_pr_detail').length);
 const countBefore=await detailCalls();const started=performance.now();await delay(125000);assert.equal(await detailCalls(),countBefore);result.tail.unchangedDetail={elapsedMs:performance.now()-started,before:countBefore,after:await detailCalls()};
 await pages[1].getByRole('button',{name:'Back to list',exact:true}).click();assert.deepEqual(await mountedReadyIdentities(pages[1]),eligible);result.tail.pairedDOM=await mountedReadyIdentities(pages[1]);
 result.tail.completedAt=new Date().toISOString();
}
