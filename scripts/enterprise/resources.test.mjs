import assert from 'node:assert/strict';
import test from 'node:test';
import {validateResourceResult} from './resources.mjs';
import {fixture} from './resource-fixtures.mjs';
test('resource phase summaries reconcile raw evidence including absent WAL and unsupported heap',()=>{validateResourceResult(fixture());validateResourceResult(fixture('chromium'));});
test('resource rejects detached, forged, missing, lost or incompatible phase evidence',()=>{
 for(const mutate of [r=>delete r.resources.evidence,r=>r.resources.summary.phases.startup.nativeRssPeak=101,r=>r.resources.evidence.native.pop(),r=>r.resources.evidence.lost=true,r=>r.resources.evidence.samplerIntervalMs=0,r=>r.resources.evidence.native[0].resource.nativeRssInclusive={status:'unknown'},r=>r.resources.evidence.contract='mixed',r=>r.resources.evidence.native[1].seq=1]){
  const r=fixture();mutate(r);assert.throws(()=>validateResourceResult(r));
 }
 const r=fixture('chromium');r.resources.evidence.browser[0].heap={status:'unknown'};assert.throws(()=>validateResourceResult(r));
});
