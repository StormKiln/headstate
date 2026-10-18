import {validateStartupEvidence} from './resources.mjs';
import assert from 'node:assert/strict';
import {assertCompatibleEnvironment,assertCompatibleWorkload,normalizeEnvironment} from './workload.mjs';

export const requiredMetrics=Object.freeze(['firstUsefulQueue','fullQueue','firstUsefulDetail','draftReadyMs','freshActionReadyMs','uiCommits','uiCommitMs']);
export const chromiumMetrics=Object.freeze([...requiredMetrics,'longTaskCount','longTaskMs']);
export const canonicalBaselineIdentities=Object.freeze(['chromium','webkit'].flatMap(engine=>[false,true].flatMap(warm=>['desktop','paired'].map(role=>`${engine}/${warm}/${role}`))));
const finiteNonnegative=(value,label)=>assert.ok(Number.isFinite(value)&&value>=0,`${label} must be finite and nonnegative`);

export function validateSamples(result){
 assert.ok(Array.isArray(result.samples),'samples required');
 const roles=result.samples.map(sample=>sample?.role);
 assert.deepEqual([...roles].sort(),['desktop','paired'],'sample roles must be exactly desktop and paired');
 assert.equal(new Set(roles).size,roles.length,'duplicate sample role');
 const metrics=result.engine==='chromium'?chromiumMetrics:requiredMetrics;
 for(const sample of result.samples)for(const metric of metrics)finiteNonnegative(sample[metric],`${sample.role} ${metric}`);
 if(result.engine==='webkit'){
  assert.deepEqual(result.longTaskCapability?.supported,false,'WebKit long tasks must be explicitly unsupported');
  for(const sample of result.samples)for(const metric of ['longTaskCount','longTaskMs'])assert.equal(sample[metric],null,`WebKit unsupported ${metric} must be null`);
 }
 finiteNonnegative(result.providerReceipts,'providerReceipts');
 return result.samples;
}

export function validateBaseline(baseline){
 assert.equal(baseline?.schema,2,'baseline schema 2 required; historical schema is incompatible');
 assert.ok(baseline.environments&&typeof baseline.environments==='object','baseline environments required');
 assert.deepEqual(Object.keys(baseline.environments).sort(),['chromium','webkit'],'baseline environments must be exactly chromium and webkit');
 for(const engine of ['chromium','webkit']){
  const environment=normalizeEnvironment(baseline.environments[engine]);
  assert.equal(environment.browser.engine,engine,`baseline ${engine} environment browser engine must match`);
 }
 assert.ok(Array.isArray(baseline.summaries)&&baseline.summaries.length>0,'baseline summaries required');
 const identities=new Set();
 for(const row of baseline.summaries){
  const identity=`${row.engine}/${row.warm}/${row.role}`;
  assert.ok(!identities.has(identity),`duplicate baseline row ${identity}`);identities.add(identity);
  assert.ok(['chromium','webkit'].includes(row.engine),`unexpected baseline engine ${row.engine}`);
  assert.equal(typeof row.warm,'boolean',`${identity} warm must be boolean`);
  assert.ok(['desktop','paired'].includes(row.role),`unexpected baseline role ${row.role}`);
  assert.ok(row.budgets&&typeof row.budgets==='object'&&Object.keys(row.budgets).length>0,`${identity} budgets required`);
  const metrics=row.engine==='chromium'?chromiumMetrics:requiredMetrics;
  assert.deepEqual(Object.keys(row.budgets).sort(),[...metrics].sort(),`${identity} budgets must contain exact required metrics`);
  for(const metric of metrics)finiteNonnegative(row.budgets[metric],`${identity} budget ${metric}`);
  finiteNonnegative(row.documentBudget,`${identity} documentBudget`);
 }
 assert.deepEqual([...identities].sort(),[...canonicalBaselineIdentities].sort(),'baseline identities must be exactly chromium and webkit, cold and warm, desktop and paired');
 return baseline;
}
// Startup-only populations: whole fault/soak request totals are not comparable.
export function compareBaseline(result,baseline){
 validateBaseline(baseline);assertCompatibleWorkload(result.workload,baseline.workload);assertCompatibleEnvironment(result.environment,baseline.environments[result.engine]);validateSamples(result);validateStartupEvidence(result);
 const matching=baseline.summaries.filter(r=>r.engine===result.engine&&r.warm===result.warm);
 assert.deepEqual(matching.map(row=>row.role).sort(),['desktop','paired'],'matching baseline rows must contain exact roles');
 return result.samples.map(sample=>{
  const row=matching.find(r=>r.role===sample.role);
  assert.ok(row,'matching measured baseline row');
  for(const [metric,limit] of Object.entries(row.budgets))assert.ok(Number.isFinite(sample[metric])&&sample[metric]<=limit,`${sample.role} ${metric}: ${sample[metric]} exceeds ${limit}`);
  assert.ok(result.providerReceipts<=row.documentBudget,`startup documents ${result.providerReceipts} exceed ${row.documentBudget}`);
  return {role:sample.role,timingBudgets:row.budgets,documentBudget:row.documentBudget};
 });
}
