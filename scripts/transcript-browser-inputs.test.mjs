import { test } from 'node:test';
import assert from 'node:assert/strict';
import { selectInputs, growthChunks, qualifyGrowth } from './transcript-browser-inputs.mjs';
const files = ['messages-1k.window-end.json', 'tool-heavy-70mb.window-end.json', 'x.window-middle.json', 'old.messages-whole.json'];
test('selects production windows and refuses missing or empty selected phases', () => {
  const got = selectInputs(files, {});
  assert.deepEqual(got.fixtures, ['messages-1k.window-end', 'tool-heavy-70mb.window-end']);
  assert.equal(got.b5.length, 3);
  for (const env of [{HARNESS_PHASES:''}, {HARNESS_PHASES:'bogus'}, {B4_FIXTURES:'messages-1k.window-end,absent'}, {GROW_FIXTURE:'absent'}, {B4_FIXTURES:''}]) {
    assert.throws(() => selectInputs(files, env));
  }
  assert.throws(() => selectInputs([], {HARNESS_PHASES:'b5'}));
  assert.throws(() => selectInputs(['x.window-middle.json'], {HARNESS_PHASES:'open'}));
  assert.equal(selectInputs(['x.window-middle.json'], {HARNESS_PHASES:'b5'}).b5.length, 1);
});
test('growth adapts to actual retained messages and must prove eviction', () => {
  assert.equal(growthChunks(10), 320);
  assert.equal(growthChunks(100), 32);
  assert.throws(() => growthChunks(0));
  assert.throws(() => qualifyGrowth(100, 1800, 18, new Set([18]), false));
  assert.throws(() => qualifyGrowth(100, 3200, 32, new Set(Array.from({length:32},(_,i)=>i+1)), false));
  assert.throws(() => qualifyGrowth(100, 3200, 32, new Set([1,32]), false));
  assert.throws(() => qualifyGrowth(100, 3200, 32, new Set(), false));
  assert.throws(() => qualifyGrowth(100, 3200, 32, new Set([30]), false));
  assert.doesNotThrow(() => qualifyGrowth(100, 3200, 32, new Set([31,32]), false));
  assert.doesNotThrow(() => qualifyGrowth(200, 2000, 20, new Set([1,19]), true));
  assert.throws(() => qualifyGrowth(200, 2000, 20, new Set([1,20]), true));
});
