import { test } from 'node:test';
import assert from 'node:assert/strict';
import { selectInputs, growthChunks, qualifyGrowth, qualifyBandwidth } from './transcript-browser-inputs.mjs';
const files = ['messages-1k.window-end.json', 'tool-heavy-70mb.window-end.json', 'x.window-middle.json', 'old.messages-whole.json'];
test('selects production windows and refuses missing or empty selected phases', () => {
  const got = selectInputs(files, {});
  assert.deepEqual(got.fixtures, ['messages-1k.window-end', 'tool-heavy-70mb.window-end', 'x.window-middle']);
  assert.equal(got.b5.length, 3);
  for (const env of [{HARNESS_PHASES:''}, {HARNESS_PHASES:'bogus'}, {B4_FIXTURES:'messages-1k.window-end,absent'}, {GROW_FIXTURE:'absent'}, {B4_FIXTURES:''}, {B4_FIXTURES:'x.window-middle'}, {GROW_FIXTURE:'x.window-middle'}]) {
    assert.throws(() => selectInputs(files, env));
  }
  assert.throws(() => selectInputs([], {HARNESS_PHASES:'b5'}));
  assert.deepEqual(selectInputs(['x.window-middle.json'], {HARNESS_PHASES:'open'}).fixtures, ['x.window-middle']);
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

test('opening includes all twelve production positions without historical stress inputs', () => {
  const windows = ['messages-1k','messages-10k','tool-heavy-70mb','huge-result-5mb'].flatMap(f => ['end','middle','start'].map(p => `${f}.window-${p}.json`));
  const got = selectInputs([...windows, 'messages-1k.messages-whole.json'], {});
  assert.equal(got.fixtures.length, 12);
  assert.equal(got.b5.length, 12);
  assert.deepEqual(got.b4Fixtures, ['messages-1k.window-end','tool-heavy-70mb.window-end']);
});

test('bandwidth estimates over the declared budget fail, not just print OVER', () => {
  assert.doesNotThrow(() => qualifyBandwidth(100_000, 2.11));
  assert.throws(() => qualifyBandwidth(400_000, 2.11), /150000/);
  for(const ratio of [0, -1, NaN]) assert.throws(() => qualifyBandwidth(100, ratio));
});
