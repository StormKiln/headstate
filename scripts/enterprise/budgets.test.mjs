import {fixture} from './resource-fixtures.mjs';
import {test} from 'node:test';
import assert from 'node:assert/strict';
import {compareBaseline,validateBaseline} from './budgets.mjs';
import {defaultWorkload} from './workload.mjs';
const metrics={firstUsefulQueue:90,fullQueue:90,firstUsefulDetail:90,draftReadyMs:90,freshActionReadyMs:90,uiCommits:2,uiCommitMs:90,longTaskCount:null,longTaskMs:null};
const environment={platform:'darwin',architecture:'arm64',osRelease:'25.6.0',cpu:'Example CPU',build:'debug profiling',strictMode:false,browser:{engine:'webkit',version:'26.0'},sourceRevision:'after'};
const row=(engine,warm,role)=>({engine,warm,role,budgets:Object.fromEntries(Object.keys(metrics).filter(k=>!k.startsWith('longTask')).map(k=>[k,100])),documentBudget:4});
const chromiumEnvironment={...environment,browser:{engine:'chromium',version:'140.0'}};
const summaries=['chromium','webkit'].flatMap(engine=>[false,true].flatMap(warm=>['desktop','paired'].map(role=>{
 const value=row(engine,warm,role);
 return engine==='chromium'?{...value,budgets:{...value.budgets,longTaskCount:5,longTaskMs:100}}:value;
})));
const baseline={schema:2,workload:defaultWorkload,environments:{chromium:{...chromiumEnvironment,sourceRevision:'before'},webkit:{...environment,sourceRevision:'before'}},summaries};
const result={engine:'webkit',warm:false,workload:defaultWorkload,environment,samples:[{role:'desktop',...metrics},{role:'paired',...metrics}],providerReceipts:4,longTaskCapability:{supported:false,value:null,reason:'unsupported'}};
const rawStartup=engine=>Object.fromEntries(['desktop','paired'].map(role=>[role,{measurement:{lost:false},firstUsefulQueue:90,fullQueue:90,detail:{start:10,visible:100,draft:100,action:100},commits:[{actual:40},{actual:50}],longTasks:engine==='chromium'?[]:null}]));
result.resources={...fixture().resources,startup:rawStartup('webkit')};
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
 assert.throws(()=>compareBaseline(result,{...baseline,summaries:baseline.summaries.slice(1)}),/baseline identities/);
 assert.throws(()=>compareBaseline(result,{...baseline,summaries:[...baseline.summaries,baseline.summaries[0]]}),/duplicate baseline row/);
 assert.throws(()=>compareBaseline(result,{...baseline,schema:1}),/baseline schema 2/);
});

test('preflight requires the exact canonical baseline identities and engine environments',()=>{
 const selectedMissing=baseline.summaries.filter(row=>!(row.engine==='webkit'&&row.warm===false&&row.role==='paired'));
 assert.throws(()=>validateBaseline({...baseline,summaries:selectedMissing}),/baseline identities/);
 assert.throws(()=>validateBaseline({...baseline,summaries:[...baseline.summaries,{...baseline.summaries[0],engine:'gecko'}]}),/baseline (?:engine|identities)/);
 assert.throws(()=>validateBaseline({...baseline,environments:{webkit:baseline.environments.webkit}}),/baseline environments/);
 assert.throws(()=>validateBaseline({...baseline,environments:{...baseline.environments,gecko:baseline.environments.webkit}}),/baseline environments/);
});

test('Chromium requires finite long-task measurements',()=>{
 const chromium={...result,resources:{...fixture('chromium').resources,startup:rawStartup('chromium')},engine:'chromium',environment:chromiumEnvironment,longTaskCapability:{supported:true},samples:result.samples.map(s=>({...s,longTaskCount:0,longTaskMs:0}))};
 assert.equal(compareBaseline(chromium,baseline).length,2);
 assert.throws(()=>compareBaseline({...chromium,samples:chromium.samples.map(s=>({...s,longTaskCount:null}))},baseline),/longTaskCount/);
});

test('baseline rejects detached numeric summaries without raw phase evidence',()=>{
 const missing={...result};delete missing.resources;assert.throws(()=>compareBaseline(missing,baseline),/raw phase/);
 const forged=structuredClone(result);forged.samples[0].firstUsefulDetail=80;assert.throws(()=>compareBaseline(forged,baseline),/reconcile raw/);
});
