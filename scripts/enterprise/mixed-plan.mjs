import assert from 'node:assert/strict';
export function validateMixedPhasePlan(plan) {
 assert.equal(plan?.schema,1,'mixed phase schema must be 1');
 assert.deepEqual(Object.keys(plan).sort(),['schema','segments']);
 assert.deepEqual(Object.keys(plan.segments??{}).sort(),['initial','resumed']);
 assert.deepEqual(Object.keys(plan.segments.initial??{}).sort(),['active_ingestion','churn_restart','idle','unchanged_refresh']);
 assert.deepEqual(Object.keys(plan.segments.resumed??{}),['churn_restart']);
 for(const duration of Object.values(plan.segments).flatMap(Object.values))assert.ok(Number.isSafeInteger(duration)&&duration>=5000&&duration<=3600000,'phase duration must be integral 5s..1h');
 return plan;
}
