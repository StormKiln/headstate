// Test-only bridge: preserve actual mounted producer output for native export smoke.
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
export const evidenceEnabled = Boolean(process.env.HEADSTATE_MEASUREMENT_SMOKE);
export const nodeVersion = process.version;
export function preserveMeasurement(name, value) {
  if (!evidenceEnabled) return;
  if (!['mounted-ready', 'mounted-stats', 'mounted-transcript', 'ready-cost'].includes(name)) throw new Error('Unsupported evidence category');
  const path = join(process.env.HEADSTATE_MEASUREMENT_SMOKE, `${name}.json`);
  const bytes = JSON.stringify(value, null, 2);
  try { writeFileSync(path, bytes, { flag: 'wx' }); }
  catch (error) {
    if (error?.code !== 'EEXIST' || readFileSync(path, 'utf8') !== bytes) throw new Error('Evidence write refused; preserve prior output and choose a fresh directory');
  }
}
