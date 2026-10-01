// Benchmark selection must not report success when requested cases are absent.
// Page-sized growth must actually cross the residency limit before claiming eviction.
const DEFAULT = ['messages-1k.window-end', 'tool-heavy-70mb.window-end'];
export function selectInputs(files, env) {
  const phases = new Set((env.HARNESS_PHASES ?? 'open,follow,growth,b5').split(','));
  if ([...phases].some(p => !['open','follow','growth','b5'].includes(p))) throw new Error('unknown or empty HARNESS_PHASES');
  const fixtures = files.filter(f => /\.window-(end|middle|start)\.json$/.test(f)).sort().map(f => f.slice(0,-5));
  const b5 = files.filter(f => /\.window-(end|middle|start)\.json$/.test(f)).sort();
  const b4Fixtures = env.B4_FIXTURES === undefined ? DEFAULT : env.B4_FIXTURES.split(',');
  const growthFixtures = env.GROW_FIXTURE === undefined ? DEFAULT : [env.GROW_FIXTURE];
  for (const [phase, selected, available] of [['open',fixtures,fixtures], ['follow',b4Fixtures,fixtures.filter(f=>f.endsWith('.window-end'))], ['growth',growthFixtures,fixtures.filter(f=>f.endsWith('.window-end'))], ['b5',b5,b5]]) {
    if (phases.has(phase) && (!selected.length || selected.some(f => !available.includes(f)))) throw new Error(`${phase}: missing or empty selected fixtures; nothing measured`);
  }
  return { phases, fixtures, b5, b4Fixtures, growthFixtures };
}
export function growthChunks(chunkSize) {
  if (!Number.isInteger(chunkSize) || chunkSize < 1) throw new Error('growth fixture has no messages');
  return Math.max(30, Math.ceil(3200 / chunkSize));
}
export function qualifyGrowth(initial, appended, chunks, held, detached) {
  if (initial + appended <= 2000) throw new Error('growth never exceeded the 2000-message resident bound');
  if (held.size === 0 || (!detached && !held.has(chunks))) throw new Error('growth heap probe did not observe retained messages');
  if (held.size >= chunks || (detached ? held.has(chunks) : held.has(1))) throw new Error('growth did not demonstrate eviction of the expected page');
}
export function qualifyBandwidth(bytes, ratio, budget = 150_000) {
  if (!Number.isFinite(bytes) || bytes <= 0 || !Number.isFinite(ratio) || ratio <= 0) throw new Error('bandwidth estimate unavailable');
  if (bytes / ratio > budget) throw new Error(`estimated compressed page ${Math.round(bytes / ratio)} bytes > ${budget} bytes`);
}
