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

test('browser phases reject missing, duplicate, invalid-time and boundary evidence for both engines',()=>{
 const corruptions=[
  e=>{e.browser=e.browser.filter(row=>row.boundary!=='interval');},
  e=>{e.browser.splice(3,1);},
  e=>{e.browser[1].seq=e.browser[0].seq;},
  e=>{e.browser[0].at=-1;},
  e=>{e.browser[0].at=NaN;},
  e=>{e.browser[2].at=e.browser[0].at-1;},
  e=>{e.browser[0].boundary='interval';},
  e=>{e.browser.pop();},
  e=>{e.browserClock.phases[0].start.nativeNs+=1;},
  e=>{e.browserClock.phases[0].end.beforeMs+=10000;},
 ];
 for(const engine of ['chromium','webkit'])for(const mutate of corruptions){const r=fixture(engine);mutate(r.resources.evidence);assert.throws(()=>validateResourceResult(r));}
});
