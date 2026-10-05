import assert from 'node:assert/strict';
import {writeFile} from 'node:fs/promises';
import {resolve} from 'node:path';
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms));
// Ordinary production StatsPage, hooks, demand lease and sixty-second worker.
// No direct Stats calls, query invalidation, scheduler wake, or synthetic frame.
export async function runStatsConvergence({pages,provider,result,out,control}) {
 await control('start');
 for(const page of pages)await page.getByRole('button',{name:'Statistics',exact:true}).click();
 for(const page of pages){await page.getByRole('button',{name:'7d',exact:true}).click();await page.waitForFunction(()=>window.__enterprise.querySummary().some(q=>q.kind==='stats-board'&&q.days===7&&q.observers>0&&q.status==='success'),null,{timeout:90000});}
 const snapshot=()=>Promise.all(pages.map(page=>page.evaluate(()=>window.__enterprise.querySummary().find(q=>q.kind==='stats-board'&&q.days===7&&q.observers>0))));
 const initial=await snapshot();for(const board of initial){assert.equal(board.complete,false);assert.equal(board.accumulated,50);assert.equal(board.total,250);}
 for(const [i,page]of pages.entries())await page.screenshot({path:resolve(out,`stats-initial-${i}.png`),fullPage:true});
 const normalCalls=await Promise.all(pages.map(page=>page.evaluate(()=>window.__enterprise.telemetry.filter(e=>e.name==='stats_board'&&e.kind==='call').length)));
 const started=Date.now(),ceilingMs=12*60000;const checkpoints=[];
 while(Date.now()-started<ceilingMs){
  await delay(15000);const boards=await snapshot();checkpoints.push({elapsedMs:Date.now()-started,boards,providerReceipts:provider.ledger.length});
  await writeFile(resolve(out,'stats-checkpoints.json'),JSON.stringify(checkpoints,null,2));
  console.log(JSON.stringify({statsConvergenceElapsedMs:Date.now()-started,boards:boards.map(b=>({accumulated:b.accumulated,total:b.total,complete:b.complete}))}));
  if(boards.every(b=>b.complete&&b.accumulated===250))break;
 }
 const final=await snapshot();assert.ok(final.every(b=>b.complete&&b.accumulated===250&&b.total===250),'automatic mounted convergence within predeclared 12 minute functional gate');
 const expected=new Map();for(const row of provider.data.rows.filter(r=>r.state==='MERGED'))expected.set(row.repository.nameWithOwner,(expected.get(row.repository.nameWithOwner)??0)+1);
 for(const [i,page]of pages.entries()){
  assert.deepEqual(final[i].repoCounts.map(r=>[r.repo,r.merged]).sort(),[...expected].sort());
  assert.equal(final[i].rows.reduce((sum,row)=>sum+row.prs,0),250);
  assert.equal(await page.evaluate(()=>window.__enterprise.telemetry.filter(e=>e.name==='stats_board'&&e.kind==='call').length),normalCalls[i],'no provider-board fallback or hidden normal refetch');
  assert.ok(await page.evaluate(()=>window.__enterprise.telemetry.some(e=>e.name==='stats_board_cached'&&e.ok===true)));
  await page.getByText('synthetic-lab/repo-1',{exact:true}).first().waitFor();
  await page.screenshot({path:resolve(out,`stats-complete-${i}.png`),fullPage:true});
 }
 assert.ok(result.cachedReads.length>=2);
 for(const read of result.cachedReads){assert.equal(read.status,200);read.concurrentProviderReceipts=read.providerAfter-read.providerBefore;}
 result.cacheOnlyEvidence='Cache CallIds match native command scopes; concurrent provider receipts/submissions are reported as overlap, not attributed to this read. Zero provider dispatch is separately verified by the actual cached-command native regression. No normal Stats reload occurs in this mounted phase.';
 result.statsConvergence={initial,final,elapsedMs:Date.now()-started,ceilingMs,normalCalls,cachedReads:result.cachedReads.length,trigger:'production stats-backfill-progress controller; exact active seven-day query; same mounted StatsPage',cadenceSeconds:60};
 result.phases.push({name:'automatic-mounted-stats-convergence-both-roles',applied:true,exercised:true,converged:true});
}
