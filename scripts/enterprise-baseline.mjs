import assert from 'node:assert/strict';
import {mkdir,readFile,writeFile,cp} from 'node:fs/promises';
import {spawn} from 'node:child_process';
import {resolve} from 'node:path';
const root=resolve(process.argv[2]);await mkdir(root,{recursive:true});
const results=[];
for(const engine of ['chromium','webkit'])for(let sample=1;sample<=5;sample++){
 const cold=resolve(root,`${engine}-cold-${sample}`);
 for(const warm of [false,true]){
  const out=warm?resolve(root,`${engine}-warm-${sample}`):cold;
  const args=['scripts/enterprise-run.mjs',out,engine,'baseline',...(warm?[resolve(cold,'profile')]:[])];
  const code=await new Promise(r=>{const p=spawn(process.execPath,args,{stdio:'inherit'});p.on('exit',r);});
  const result=JSON.parse(await readFile(resolve(out,'result.json'),'utf8'));results.push({...result,artifact:out});
  await writeFile(resolve(root,'samples.json'),JSON.stringify(results,null,2));assert.equal(code,0);assert.equal(result.pass,true);
  if(!warm)await cp(resolve(cold,'profile'),resolve(cold,'profile-at-cold-shutdown'),{recursive:true});
 }
}
const distribution=values=>{const a=[...values].sort((a,b)=>a-b);assert.ok(a.every(Number.isFinite));return {n:a.length,min:a[0],max:a.at(-1),median:a.length%2?a[Math.floor(a.length/2)]:(a[a.length/2-1]+a[a.length/2])/2,samples:values};};
const summaries=[];
for(const engine of ['chromium','webkit'])for(const warm of [false,true])for(const role of ['desktop','paired']){
 const runs=results.filter(r=>r.engine===engine&&r.warm===warm),samples=runs.map(r=>r.samples.find(s=>s.role===role));
 summaries.push({engine,warm,role,metrics:Object.fromEntries(['firstUsefulQueue','fullQueue','firstUsefulDetail','draftReadyMs','freshActionReadyMs','uiCommits','uiCommitMs',...(engine==='chromium'?['longTaskCount','longTaskMs']:[])].map(key=>[key,distribution(samples.map(s=>s[key]))])),providerDocuments:distribution(runs.map(r=>r.providerReceipts))});
}
const metadata=await Promise.all(results.map(r=>readFile(resolve(r.artifact,'metadata.json'),'utf8').then(JSON.parse)));
const policy={schema:1,environment:metadata[0],engines:Object.fromEntries(results.map(r=>[r.engine,r.version])),measuredCommits:[...new Set(metadata.map(m=>m.commit))],scope:'synthetic native-connected startup and detail regression baseline; no live enterprise SLA',sampleDesign:'five independent cold profiles and one warm restart of each per engine; both roles present; no robust p95 claim',timingEnvelope:'2 * observed maximum + 500ms for durations; 2 * maximum + 5 for commit/long-task counts; initial conservative same-host/build regression threshold, not a responsiveness SLA',documentEnvelope:'ceil(1.25 * observed maximum) + 4 documents; tolerates bounded advisory scheduling variation only with correct accepted inventories',summaries:summaries.map(s=>({...s,budgets:Object.fromEntries(Object.entries(s.metrics).map(([k,v])=>[k,Math.ceil(2*v.max+(k==='uiCommits'||k==='longTaskCount'?5:500))])),documentBudget:Math.ceil(1.25*s.providerDocuments.max)+4}))};
await writeFile(resolve(root,'baseline.json'),JSON.stringify(policy,null,2));
