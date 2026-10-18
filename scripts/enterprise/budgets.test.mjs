import {test} from 'node:test';
import assert from 'node:assert/strict';
import {compareBaseline} from './budgets.mjs';
import {defaultWorkload} from './workload.mjs';
const metrics={firstUsefulQueue:90,fullQueue:90,firstUsefulDetail:90,draftReadyMs:90,freshActionReadyMs:90,uiCommits:2,uiCommitMs:90,longTaskCount:null,longTaskMs:null};
const environment={platform:'darwin',architecture:'arm64',osRelease:'25.6.0',cpu:'Example CPU',build:'debug profiling',strictMode:false,browser:{engine:'webkit',version:'26.0'},sourceRevision:'after'};
const row=role=>({engine:'webkit',warm:false,role,budgets:Object.fromEntries(Object.keys(metrics).filter(k=>!k.startsWith('longTask')).map(k=>[k,100])),documentBudget:4});
const baseline={schema:2,workload:defaultWorkload,environments:{webkit:{...environment,sourceRevision:'before'}},summaries:[row('desktop'),row('paired')]};
const result={engine:'webkit',warm:false,workload:defaultWorkload,environment,samples:[{role:'desktop',...metrics},{role:'paired',...metrics}],providerReceipts:4,longTaskCapability:{supported:false,value:null,reason:'unsupported'}};
test('measured baseline accepts complete bounded samples with explicit unsupported WebKit long tasks',()=>{
 assert.equal(compareBaseline(result,baseline).length,2);
});

test('rejects missing, duplicate, unexpected, or empty role evidence',()=>{
 for(const samples of [[],[result.samples[0]],[...result.samples,result.samples[0]],[...result.samples,{...result.samples[0],role:'observer'}]])assert.throws(()=>compareBaseline({...result,samples},baseline),/role/);
});

test('required measurements do not depend on arbitrary budget keys',()=>{
 assert.throws(()=>compareBaseline({...result,samples:result.samples.map(s=>{const copy={...s};delete copy.fullQueue;return copy;})},baseline),/fullQueue/);
 for(const value of [null,-1,NaN,Infinity])assert.throws(()=>compareBaseline({...result,samples:result.samples.map(s=>({...s,uiCommitMs:value}))},baseline),/uiCommitMs/);
 assert.throws(()=>compareBaseline(result,{...baseline,summaries:baseline.summaries.map(r=>({...r,budgets:{}}))}),/budgets/);
});

test('requires finite nonnegative provider documents and budgets',()=>{
 for(const providerReceipts of [null,-1,NaN,Infinity])assert.throws(()=>compareBaseline({...result,providerReceipts},baseline),/providerReceipts/);
 for(const documentBudget of [null,-1,NaN,Infinity])assert.throws(()=>compareBaseline(result,{...baseline,summaries:baseline.summaries.map(r=>({...r,documentBudget}))}),/documentBudget/);
});

test('rejects missing and duplicate baseline rows and legacy schema explicitly',()=>{
 assert.throws(()=>compareBaseline(result,{...baseline,summaries:baseline.summaries.slice(1)}),/baseline row/);
 assert.throws(()=>compareBaseline(result,{...baseline,summaries:[...baseline.summaries,baseline.summaries[0]]}),/duplicate baseline row/);
 assert.throws(()=>compareBaseline(result,{...baseline,schema:1}),/baseline schema 2/);
});

test('Chromium requires finite long-task measurements',()=>{
 const chromiumEnvironment={...environment,browser:{engine:'chromium',version:'140.0'}};
 const chromiumRows=baseline.summaries.map(r=>({...r,engine:'chromium',budgets:{...r.budgets,longTaskCount:5,longTaskMs:100}}));
 const chromium={...result,engine:'chromium',environment:chromiumEnvironment,longTaskCapability:{supported:true},samples:result.samples.map(s=>({...s,longTaskCount:0,longTaskMs:0}))};
 const chromiumBaseline={...baseline,environments:{chromium:{...chromiumEnvironment,sourceRevision:'before'}},summaries:chromiumRows};
 assert.equal(compareBaseline(chromium,chromiumBaseline).length,2);
 assert.throws(()=>compareBaseline({...chromium,samples:chromium.samples.map(s=>({...s,longTaskCount:null}))},chromiumBaseline),/longTaskCount/);
});
