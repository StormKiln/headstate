import {test} from 'node:test';
import assert from 'node:assert/strict';
import {defaultWorkload,normalizeWorkload,assertCompatibleWorkload,normalizeEnvironment,assertCompatibleEnvironment} from './workload.mjs';

test('normalizes the legacy fixture workload without changing its populations',()=>{
 const workload=normalizeWorkload();
 assert.deepEqual(workload,defaultWorkload);
 assert.deepEqual(workload.expectedRoles,['desktop','paired']);
 assert.deepEqual(workload.populations,{authored:50,reviewing:150,repositories:50,members:50,history:250});
 assert.equal(Object.isFrozen(workload),true);
 assert.equal(Object.isFrozen(workload.populations),true);
});

test('rejects incomplete, duplicate, or unexpected role contracts',()=>{
 for(const expectedRoles of [[],['desktop'],['desktop','desktop'],['desktop','paired','observer']]){
  assert.throws(()=>normalizeWorkload({...defaultWorkload,expectedRoles}),/expectedRoles/);
 }
});

test('requires finite nonnegative populations and cadence',()=>{
 for(const value of [undefined,null,-1,NaN,Infinity])assert.throws(()=>normalizeWorkload({...defaultWorkload,populations:{...defaultWorkload.populations,reviewing:value}}),/reviewing/);
 for(const cadenceSeconds of [null,-1,NaN,Infinity])assert.throws(()=>normalizeWorkload({...defaultWorkload,cadenceSeconds}),/cadenceSeconds/);
});

test('workload admission covers every behavioral field',()=>{
 const expected=normalizeWorkload();
 const mutations={seed:1,scenario:'churn',cadenceSeconds:30,scheduleVersion:'v2'};
 for(const [field,value] of Object.entries(mutations))assert.throws(()=>assertCompatibleWorkload({...expected,[field]:value},expected),new RegExp(field));
 for(const field of ['id','hash'])assert.throws(()=>assertCompatibleWorkload({...expected,quotaProfile:{...expected.quotaProfile,[field]:'changed'}},expected),new RegExp(`quotaProfile.${field}`));
 for(const field of Object.keys(expected.populations))assert.throws(()=>assertCompatibleWorkload({...expected,populations:{...expected.populations,[field]:expected.populations[field]+1}},expected),new RegExp(field));
});

test('environment admission covers host browser and build but permits source revision changes',()=>{
 const expected=normalizeEnvironment({platform:'darwin',architecture:'arm64',osRelease:'25.6.0',cpu:'Example CPU',build:'debug profiling',strictMode:false,browser:{engine:'chromium',version:'140.0'},sourceRevision:'before'});
 assert.doesNotThrow(()=>assertCompatibleEnvironment({...expected,sourceRevision:'after'},expected));
 for(const [field,value] of Object.entries({platform:'linux',architecture:'x64',osRelease:'other',cpu:'other',build:'release',strictMode:true}))assert.throws(()=>assertCompatibleEnvironment({...expected,[field]:value},expected),new RegExp(field));
 assert.throws(()=>assertCompatibleEnvironment({...expected,browser:{...expected.browser,version:'141.0'}},expected),/browser.version/);
});
