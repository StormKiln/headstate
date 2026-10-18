import test from 'node:test';import assert from 'node:assert/strict';import {assertMixedCoverage} from './mixed-coverage.mjs';
import {coverageWitness as witness} from './mixed-test-fixtures.mjs';

test('mounted coverage requires independent baseline/degraded/paused/recovered facts and corresponding text',()=>{for(const warm of [false,true])for(const stage of ['baseline','degraded','paused','recovered'])assertMixedCoverage(witness(stage,warm));});
test('valid FTS/native facts cannot hide stale, falsely complete or unrendered coverage',()=>{for(const corrupt of [w=>w.coverage.reply.sessions=0,w=>w.indexCoverage.reply.population.status='complete',w=>w.renderedIndex.truncated=0,w=>w.text=w.text.replace('coverage is not confirmed complete.','All 131 sessions in the last observed corpus are searchable.'),w=>w.text='session-001 valid FTS hit',w=>w.coverage.ok=false,w=>w.indexCoverage.reply.indexed=0]){const w=witness();corrupt(w);assert.throws(()=>assertMixedCoverage(w));}});
