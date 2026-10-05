import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import {resolve} from 'node:path';
import {advisoryPeriods,usefulPublications} from './ready-completion.mjs';
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms));
export const soakPolicy={minimumMs:1800000,historyBudgetMs:1200000,interestMarginMs:180000,usefulBlockMs:120000,finalMarginMs:300000,flatProgressMs:600000,checkpointMs:60000,meaning:'conditional synthetic diagnostic ceiling, not a universal liveness guarantee or SLA'};
export function remainingWork(coverage,expected){return {pushers:expected.filter(n=>!coverage.pushers.includes(n)||!coverage.rules.includes(n)),stacks:expected.filter(n=>!coverage.stacks.includes(n))};}
export function diagnosticCeiling(remaining){return Math.max(soakPolicy.minimumMs,soakPolicy.historyBudgetMs,soakPolicy.interestMarginMs+soakPolicy.usefulBlockMs*Math.max(remaining.pushers.length,remaining.stacks.length)+soakPolicy.finalMarginMs);}
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
export async function statsDOM(page){return page.evaluate(()=>({
 repos:[...document.querySelectorAll('button')].filter(n=>n.querySelector('span[title^="synthetic-lab/repo-"]')).map(n=>{const spans=[...n.children];return {repo:spans[0].textContent,count:Number(spans[2].textContent.replaceAll(',','')),share:spans[3].textContent};}).sort((a,b)=>a.repo.localeCompare(b.repo)),
 completeHint:[...document.querySelectorAll('*')].some(n=>n.childNodes.length===1&&n.textContent==='share of all 250 merged in this window'),
 impossibleHint:/\b\d+ of 0 merged/.test(document.body.textContent??'')
}));}
export function assertStatsMeasurement(board,dom,history,now=Date.now()){
 const today=new Date(now);today.setUTCHours(0,0,0,0);
 assert.deepEqual(board.window,{from:new Date(today.getTime()-30*86400000).toISOString().slice(0,10),to:new Date(today.getTime()-86400000).toISOString().slice(0,10)},'actual current closed thirty-day question');
 assert.equal(board.complete,true);assert.equal(board.accumulated,250);assert.equal(board.total,250);assert.equal(board.days,30);assert.equal(board.daysCovered,30);assert.equal(board.daysTotal,30);
 assert.equal(board.owner?.viewer,'synthetic-viewer');assert.equal(board.scopeKey,'merged|*|org:synthetic-lab');
 const repos=new Map(),authors=new Map();for(const row of history){repos.set(row.repository.nameWithOwner,(repos.get(row.repository.nameWithOwner)??0)+1);authors.set(row.author.login,(authors.get(row.author.login)??0)+1);}
 assert.deepEqual(board.repoCounts.map(r=>[r.repo,r.merged]).sort(),[...repos].sort());assert.deepEqual(board.rows.map(r=>[r.login,r.prs]).sort(),[...authors].sort());
 assert.deepEqual(dom.repos.map(r=>[r.repo,r.count]).sort(),[...repos].sort());assert.ok(dom.repos.every(r=>r.share==='2%'));assert.equal(dom.completeHint,true);assert.equal(dom.impossibleHint,false);
}
export async function runEnterpriseSoak({pages,provider,result,out,profile,nativeCall}){
 const expectedNumbers=Array.from({length:148},(_,i)=>51+i),open=provider.data.rows.filter(r=>r.state==='OPEN');
 const authored=open.filter(r=>r.author.login==='synthetic-viewer').map(r=>`${r.repository.nameWithOwner}/${r.number}`).sort();
 const reviewing=open.filter(r=>expectedNumbers.includes(r.number)).map(r=>`${r.repository.nameWithOwner}/${r.number}`).sort();
 const readNative=async()=>{const text=await readFile(resolve(profile,'native.ndjson'),'utf8');return text.slice(0,text.lastIndexOf('\n')).split('\n').filter(Boolean).map(JSON.parse);};
 const snapshot=page=>page.evaluate(()=>({inventory:window.__enterprise.inventory(),queries:window.__enterprise.querySummary(),publications:window.__enterprise.advisoryPublications,counts:document.querySelector('[data-testid="counts"]')?.textContent,lost:window.__enterprise.measurement.lost,cachedCalls:window.__enterprise.telemetry.filter(e=>e.kind==='call'&&e.name==='stats_board_cached').length,normalStatsCalls:window.__enterprise.telemetry.filter(e=>e.kind==='call'&&e.name==='stats_board').length}));
 await pages[0].getByRole('button',{name:'Back to list',exact:true}).click();
 const initial=await snapshot(pages[0]),initialStats=await snapshot(pages[1]);
 const initialCoverage=usefulPublications(initial.publications,expectedNumbers),remaining=remainingWork(initialCoverage,expectedNumbers),ceilingMs=diagnosticCeiling(remaining);
 const start=performance.now();result.soak={...soakPolicy,ceilingMs,initialRemaining:remaining,initialCoverage,initialStats:initialStats.queries.find(q=>q.kind==='stats-board'&&q.observers>0),checkpoints:[],obligations:{externalClose:false,historyMeasured:false,advisoryObserved:false,reviewGuidanceObserved:false,mountedStats:false,settledStatsReadback:false}};
 const save=()=>writeFile(resolve(out,'soak-progress.json'),JSON.stringify(result.soak,null,2));
 await writeFile(resolve(out,'soak-policy.json'),JSON.stringify({source:JSON.parse(await readFile(resolve(out,'metadata.json'),'utf8')),policy:soakPolicy,remaining,ceilingMs,assumptions:['one desktop ReadyStrip; paired continuously Stats','unchanged finite heads/bases and healthy bounded synthetic responses','same one-minute moving viewport and actual30s windows','<=2 documents per cold advisory command; unchanged8-attempt allowance/TTLs','120s per-new-identity planning block qualified by16-row run, not a proof','10min flat useful progress with continuing admission is an earlier diagnostic FAIL']},null,2));
 await save();assert.equal(result.soak.initialStats?.complete,false,'mounted Stats must start partial');
 provider.fault.closed=[200];provider.fault.merged=[199];
 let lastNew={pushers:0,stacks:0},lastSize={pushers:remaining.pushers.length,stacks:remaining.stacks.length},lastDebit={pushers:0,stacks:0},lastCheckpoint=-1,statsSettled;
 try{
  while(performance.now()-start<ceilingMs){
   await delay(Math.min(5000,Math.max(0,ceilingMs-(performance.now()-start))));
   const minute=Math.floor((performance.now()-start)/soakPolicy.checkpointMs);if(minute===lastCheckpoint)continue;lastCheckpoint=minute;
   const rows=pages[0].locator('[data-advisory-key]'),count=await rows.count();if(count)await rows.nth((minute*8)%count).scrollIntoViewIfNeeded();
   const ui=await snapshot(pages[0]),paired=await snapshot(pages[1]),dom=await statsDOM(pages[1]);
   const native=await readNative(),debits=native.filter(e=>e.operation==='advisory-share'&&e.stage==='debit'),lastNs=native.at(-1)?.ns??0,recentDebit=debits.at(-1)?.ns??0;
   const coverage=usefulPublications(ui.publications,expectedNumbers),owed=remainingWork(coverage,expectedNumbers);
   const board=paired.queries.find(q=>q.kind==='stats-board'&&q.observers>0);
   const db=JSON.parse(execFileSync('python3',['scripts/enterprise/inspect-profile.py',profile],{encoding:'utf8'}));
   const displayed=await mountedReadyIdentities(pages[0]);
   const exact=(a,b)=>JSON.stringify(a)===JSON.stringify(b);
   result.soak.obligations.externalClose=exact(ui.inventory.authored,authored)&&exact(paired.inventory.authored,authored)&&exact(ui.inventory.reviewing,reviewing)&&exact(paired.inventory.reviewing,reviewing)&&exact(displayed,reviewing);
   result.soak.obligations.advisoryObserved=owed.pushers.length===0;result.soak.obligations.reviewGuidanceObserved=owed.stacks.length===0;
   const history=provider.data.rows.filter(r=>r.state==='MERGED'&&r.mergedAt.slice(0,10)>=board?.window?.from&&r.mergedAt.slice(0,10)<=board?.window?.to);
   const actualHistory=db.historyIdentities.filter(r=>r.scope===board?.scopeKey&&r.day.slice(0,10)>=board?.window?.from&&r.day.slice(0,10)<=board?.window?.to).map(r=>`${r.repo}/${r.number}`).sort();
   const expectedHistory=history.map(r=>`${r.repository.nameWithOwner}/${r.number}`).sort();
   result.soak.obligations.historyMeasured=history.length===250&&exact(actualHistory,expectedHistory)&&db.historyCoverage.some(s=>s.scope===board?.scopeKey&&s.horizonDays===30&&s.missingRequiredDays===0);
   const elapsedMs=performance.now()-start;
   const checkpoint={elapsedMs,owed,coverage,displayEvidence:displayEvidence(ui.queries,expectedNumbers),inventory:{desktop:ui.inventory,paired:paired.inventory,displayed},queries:ui.queries,stats:board,statsDOM:dom,db,providerReceipts:provider.ledger.length,advisoryDebits:debits.length,nativePeriods:advisoryPeriods(native),cachedCalls:paired.cachedCalls};
   result.soak.checkpoints.push(checkpoint);await save();
   assertAdvisoryPeriods(checkpoint.nativePeriods);
   assert.equal(ui.lost||paired.lost,false);assert.equal(db.integrity,'ok');assert.equal(dom.impossibleHint,false);assert.equal(paired.normalStatsCalls,initialStats.normalStatsCalls,'automatic cache bridge cannot fall back to normal provider board loads');
   if(board?.complete){
    assertStatsMeasurement(board,dom,history);result.soak.obligations.mountedStats=true;
    if(!statsSettled){statsSettled={elapsedMs,cachedCalls:paired.cachedCalls};await pages[1].getByText('share of all 250 merged in this window',{exact:true}).scrollIntoViewIfNeeded();await pages[1].screenshot({path:resolve(out,'soak-stats-complete.png')});}
    else if(elapsedMs-statsSettled.elapsedMs>=125000){assert.equal(paired.cachedCalls,statsSettled.cachedCalls,'settled countdown/duplicate frames issue no new cached command');result.soak.obligations.settledStatsReadback=true;}
   }
   for(const family of ['pushers','stacks']){
    if(owed[family].length<lastSize[family]){lastNew[family]=elapsedMs;lastSize[family]=owed[family].length;lastDebit[family]=debits.length;}
    if(owed[family].length&&elapsedMs>soakPolicy.interestMarginMs&&elapsedMs-Math.max(lastNew[family],soakPolicy.interestMarginMs)>=soakPolicy.flatProgressMs&&debits.length-lastDebit[family]>=8&&lastNs-recentDebit<120e9){
     result.soak.flatProgressFailure={family,owed:owed[family],lastNewMs:lastNew[family],elapsedMs,admissionContinues:true};await save();assert.fail('10 minutes without a new useful unchanged eligible identity despite continuing admission');
    }
   }
   console.log(JSON.stringify({soakElapsedMs:elapsedMs,ceilingMs,pushersRemaining:owed.pushers.length,stacksRemaining:owed.stacks.length,statsComplete:board?.complete,historyComplete:result.soak.obligations.historyMeasured}));
   if(Object.values(result.soak.obligations).every(Boolean)&&result.soak.firstFullConvergenceMs===undefined)result.soak.firstFullConvergenceMs=elapsedMs;
   await save();if(elapsedMs>=soakPolicy.minimumMs&&Object.values(result.soak.obligations).every(Boolean))break;
  }
  result.soak.elapsedMs=performance.now()-start;await save();
  assert.ok(result.soak.elapsedMs>=soakPolicy.minimumMs&&result.soak.elapsedMs<=ceilingMs);assert.ok(Object.values(result.soak.obligations).every(Boolean),'all exact population/DOM/history/useful obligations before immutable diagnostic ceiling');
  const before=provider.ledger.length;for(let i=0;i<5;i++){const response=await nativeCall('paired','stats_board',{scopeKind:'org',scopeValue:'synthetic-lab',measure:'merged',days:30});assert.equal(response.status,200);const board=(await response.json()).wire;assert.equal(board.complete,true);assert.equal(board.total,250);}assert.equal(provider.ledger.length,before);result.soak.completeCacheReads=5;
  await pages[1].getByRole('button',{name:'To Review',exact:true}).click();await pages[1].getByText('Synthetic description 52',{exact:false}).waitFor();await delay(3000);
  const detailCalls=()=>pages[1].evaluate(()=>window.__enterprise.telemetry.filter(e=>e.kind==='call'&&e.name==='get_pr_detail').length);
  const countBefore=await detailCalls();await delay(125000);assert.equal(await detailCalls(),countBefore);result.soak.unchangedDetailWindowMs=125000;
  await pages[1].getByRole('button',{name:'Back to list',exact:true}).click();assert.deepEqual(await mountedReadyIdentities(pages[1]),reviewing,'actual paired final Ready inventory');
  await save();
 }finally{result.soak.totalWithFinalPhaseMs=performance.now()-start;await save();}
}
