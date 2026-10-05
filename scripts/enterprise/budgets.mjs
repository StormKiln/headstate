import assert from 'node:assert/strict';
// Startup-only populations: whole fault/soak request totals are not comparable.
export function compareBaseline(result,baseline){
 return result.samples.map(sample=>{
  const row=baseline.summaries.find(r=>r.engine===result.engine&&r.warm===result.warm&&r.role===sample.role);
  assert.ok(row,'matching measured baseline row');
  for(const [metric,limit] of Object.entries(row.budgets))assert.ok(Number.isFinite(sample[metric])&&sample[metric]<=limit,`${sample.role} ${metric}: ${sample[metric]} exceeds ${limit}`);
  assert.ok(result.providerReceipts<=row.documentBudget,`startup documents ${result.providerReceipts} exceed ${row.documentBudget}`);
  return {role:sample.role,timingBudgets:row.budgets,documentBudget:row.documentBudget};
 });
}
