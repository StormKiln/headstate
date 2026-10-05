import {test} from 'node:test';import assert from 'node:assert/strict';
import {mkdtemp,mkdir,writeFile,cp,rm} from 'node:fs/promises';import {tmpdir} from 'node:os';import {join} from 'node:path';
import {fixture} from './provider.mjs';
import {baselineRows,restoreBaseline,loadConvergenceRestart,captureRestartCache} from './convergence-restart.mjs';
import {initialConvergenceExpected} from './convergence.mjs';
test('restart replays genuinely older facts and preserves repository references',()=>{
 const data=fixture({convergence:true}),baseline=baselineRows(data),row=data.rows.find(row=>row.number===51),repo=row.repository;
 row.state='MERGED';row.updatedAt='2099-01-01T00:00:00Z';restoreBaseline(data,baseline);
 assert.equal(row.state,'OPEN');assert.equal(row.updatedAt,baseline.find(row=>row.number===51).updatedAt);assert.equal(row.repository,repo);
 assert.throws(()=>restoreBaseline(data,baseline.slice(1)));
});
test('restart requires successful retained artifacts and byte identical copied profile',async()=>{
 const dir=await mkdtemp(join(tmpdir(),'convergence-restart-'));try{
  const profile=join(dir,'profile'),copied=join(dir,'copy');await mkdir(profile);await writeFile(join(profile,'.enterprise-synthetic'),'test');await writeFile(join(profile,'headstate.db'),'original bytes');await cp(profile,copied,{recursive:true});
  await writeFile(join(dir,'convergence.json'),JSON.stringify({pass:true,expected:initialConvergenceExpected()}));await writeFile(join(dir,'convergence-baseline.json'),JSON.stringify(baselineRows(fixture({convergence:true}))));await writeFile(join(dir,'result.json'),JSON.stringify({pass:true,shutdown:{clean:true},exit:{code:0}}));
  const provider={data:fixture({convergence:true}),fault:{}};const state=await loadConvergenceRestart(profile,copied,provider);assert.equal(state.expected.reviewing.length,236);assert.ok(state.profileSha256['headstate.db']);assert.equal(provider.fault.holdSearch.remaining,1);
  await writeFile(join(copied,'headstate.db'),'changed');await assert.rejects(loadConvergenceRestart(profile,copied,provider),/byte identical/);
 }finally{await rm(dir,{recursive:true,force:true});}
});
test('pre-mount cache proof rejects provider IO and wrong native identity envelope',async()=>{
 const dir=await mkdtemp(join(tmpdir(),'convergence-cache-'));try{
  const expected=initialConvergenceExpected(),provider={ledger:[]};const nativeCall=async(role,_command,{list})=>({status:200,json:async()=>({[role==='desktop'?'value':'wire']:{ownership:{state:'credential_bound'},data:{state:'available',prs:expected[list].map(id=>({repo:id.slice(0,id.lastIndexOf('/')),number:Number(id.slice(id.lastIndexOf('/')+1))}))}}})});
  assert.equal((await captureRestartCache({nativeCall,provider,state:{expected},out:dir})).cache.length,4);
  await assert.rejects(captureRestartCache({nativeCall:async(...args)=>{provider.ledger.push({});return nativeCall(...args);},provider,state:{expected},out:dir}),/no provider IO/);
 }finally{await rm(dir,{recursive:true,force:true});}
});
