import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
const helper = new URL('./measurement-evidence.mjs', import.meta.url).href;
function run(code, root) {
  const env = { ...process.env };
  delete env.HEADSTATE_MEASUREMENT_SMOKE;
  if (root) env.HEADSTATE_MEASUREMENT_SMOKE = root;
  return spawnSync(process.execPath, ['--input-type=module', '-e', `import { preserveMeasurement, evidenceEnabled } from ${JSON.stringify(helper)}; ${code}`], { env, encoding: 'utf8' });
}
test('ordinary tests do not write artifacts without explicit opt-in', () => {
  assert.equal(run('if(evidenceEnabled) throw Error(); preserveMeasurement("mounted-ready", [1]);').status, 0);
});
test('explicit evidence is preserved; identical output allowed, changed or unknown output refused', () => {
  const root = mkdtempSync(join(tmpdir(), 'measurement-evidence-'));
  try {
    assert.equal(run('preserveMeasurement("mounted-ready", [1]); preserveMeasurement("mounted-ready", [1]);', root).status, 0);
    assert.notEqual(run('preserveMeasurement("mounted-ready", [2]);', root).status, 0);
    assert.notEqual(run('preserveMeasurement("../private", [2]);', root).status, 0);
    assert.deepEqual(JSON.parse(readFileSync(join(root, 'mounted-ready.json'), 'utf8')), [1]);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
