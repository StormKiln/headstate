import test from 'node:test';import assert from 'node:assert/strict';
import {fixture} from './resource-fixtures.mjs';
import {summarizeMixedResources} from './mixed-resources.mjs';
const segments=()=>[{kind:'initial',warm:false,process:{pid:100,nonce:'a'},evidence:fixture('chromium','mixed_initial').resources.evidence},{kind:'resumed',warm:true,process:{pid:101,nonce:'b'},evidence:fixture('chromium','mixed_resumed').resources.evidence}];
test('mixed resources keep independent process clocks and sum only active phase durations',()=>{
 const result=summarizeMixedResources({schema:2,contract:'mixed',segments:segments()},'chromium');assert.equal(result.phases.churn_restart.durationMs,8000);assert.equal(result.phases.idle.durationMs,4000);assert.equal(result.phases.churn_restart.nativeRssPeak,100);assert.equal(result.phases.churn_restart.nativeRssWindowMedianDelta,null);assert.equal(result.phases.churn_restart.segmentMemoryDrift.length,2);
});
test('mixed resources reject missing/duplicate/foreign segments and local clock/counter corruption',()=>{
 for(const corrupt of [s=>s.pop(),s=>s.push(s[0]),s=>s[1].kind='initial',s=>s[1].process=s[0].process,s=>s[1].evidence.native[1].seq=1,s=>s[1].evidence.native.at(-1).resource.readBytes.metadata=-1,s=>s[0].evidence.lost=true]){const s=segments();corrupt(s);assert.throws(()=>summarizeMixedResources({schema:2,contract:'mixed',segments:s},'chromium'));}
});
