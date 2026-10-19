import test from 'node:test';import assert from 'node:assert/strict';import {aggregateMixedParent} from './mixed-parent.mjs';
const pair=()=>[{nativeRssPeak:100,commandLatencyMaxMs:4,lost:false},{nativeRssPeak:200,commandLatencyMaxMs:3,lost:false}];
test('mixed parent aggregates only measured per-process values',()=>assert.deepEqual(aggregateMixedParent(pair()),{nativeRssPeak:200,commandLatencyMaxMs:4,lost:false}));
test('one or both unknown parent probes cannot become measured zero or hide behind another process',()=>{for(const mutate of [p=>p[0].nativeRssPeak=null,p=>p.forEach(s=>s.nativeRssPeak=null),p=>p[1].commandLatencyMaxMs=null,p=>p[0].nativeRssPeak='100',p=>p[1].lost=true]){const p=pair();mutate(p);assert.throws(()=>aggregateMixedParent(p));}});
