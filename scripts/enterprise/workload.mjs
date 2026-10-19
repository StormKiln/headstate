import assert from 'node:assert/strict';

const roles=['desktop','paired'];
const populationFields=['authored','reviewing','repositories','members','history'];
const workloadFields=['schema','seed','scenario','expectedRoles','populations','cadenceSeconds','scheduleVersion','quotaProfile'];
const environmentFields=['platform','architecture','osRelease','cpu','build','strictMode','browser','sourceRevision'];

const finiteNonnegative=(value,label)=>assert.ok(Number.isFinite(value)&&value>=0,`${label} must be finite and nonnegative`);
const exactKeys=(input,fields,label)=>assert.deepEqual(Object.keys(input).sort(),[...fields].sort(),`${label} fields must be exact`);

export const defaultWorkload=normalizeWorkload({
 schema:1,
 seed:9001,
 scenario:'baseline',
 expectedRoles:roles,
 populations:{authored:50,reviewing:150,repositories:50,members:50,history:250},
 cadenceSeconds:60,
 scheduleVersion:'legacy-v1',
 quotaProfile:{id:'legacy-equal-document-v1',hash:'sha256:legacy-equal-document-v1'},
});

export function normalizeWorkload(input=undefined){
 if(input===undefined&&typeof defaultWorkload!=='undefined')return defaultWorkload;
 assert.ok(input&&typeof input==='object'&&!Array.isArray(input),'workload descriptor required');
 exactKeys(input,workloadFields,'workload');
 assert.equal(input.schema,1,'workload schema must be 1');
 finiteNonnegative(input.seed,'seed');
 assert.ok(typeof input.scenario==='string'&&input.scenario.length>0,'scenario required');
 assert.deepEqual(input.expectedRoles,roles,'expectedRoles must be exactly desktop and paired');
 assert.ok(input.populations&&typeof input.populations==='object'&&!Array.isArray(input.populations),'populations required');
 exactKeys(input.populations,populationFields,'population');
 for(const field of populationFields)finiteNonnegative(input.populations[field],field);
 finiteNonnegative(input.cadenceSeconds,'cadenceSeconds');
 assert.ok(typeof input.scheduleVersion==='string'&&input.scheduleVersion.length>0,'scheduleVersion required');
 assert.ok(input.quotaProfile&&typeof input.quotaProfile==='object'&&!Array.isArray(input.quotaProfile),'quotaProfile required');
 exactKeys(input.quotaProfile,['id','hash'],'quotaProfile');
 for(const field of ['id','hash'])assert.ok(typeof input.quotaProfile[field]==='string'&&input.quotaProfile[field].length>0,`quotaProfile ${field} required`);
 const populations=Object.freeze(Object.fromEntries(populationFields.map(field=>[field,input.populations[field]])));
 return Object.freeze({...input,expectedRoles:Object.freeze([...roles]),populations,quotaProfile:Object.freeze({...input.quotaProfile})});
}

export function assertCompatibleWorkload(actual,expected){
 const left=normalizeWorkload(actual),right=normalizeWorkload(expected);
 for(const field of ['schema','seed','scenario','cadenceSeconds','scheduleVersion'])assert.equal(left[field],right[field],`workload ${field} must match`);
 for(const field of ['id','hash'])assert.equal(left.quotaProfile[field],right.quotaProfile[field],`workload quotaProfile.${field} must match`);
 assert.deepEqual(left.expectedRoles,right.expectedRoles,'workload expectedRoles must match');
 for(const field of populationFields)assert.equal(left.populations[field],right.populations[field],`workload population ${field} must match`);
 return left;
}

export function normalizeEnvironment(input){
 assert.ok(input&&typeof input==='object'&&!Array.isArray(input),'environment descriptor required');
 exactKeys(input,environmentFields,'environment');
 for(const field of ['platform','architecture','osRelease','cpu','build','sourceRevision'])assert.ok(typeof input[field]==='string'&&input[field].length>0,`environment ${field} required`);
 assert.equal(typeof input.strictMode,'boolean','environment strictMode required');
 assert.ok(input.browser&&typeof input.browser==='object'&&!Array.isArray(input.browser),'environment browser required');
 exactKeys(input.browser,['engine','version'],'environment browser');
 for(const field of ['engine','version'])assert.ok(typeof input.browser[field]==='string'&&input.browser[field].length>0,`environment browser.${field} required`);
 return Object.freeze({...input,browser:Object.freeze({...input.browser})});
}

export function assertCompatibleEnvironment(actual,expected){
 const left=normalizeEnvironment(actual),right=normalizeEnvironment(expected);
 for(const field of ['platform','architecture','osRelease','cpu','build','strictMode'])assert.equal(left[field],right[field],`environment ${field} must match`);
 for(const field of ['engine','version'])assert.equal(left.browser[field],right.browser[field],`environment browser.${field} must match`);
 return left;
}
