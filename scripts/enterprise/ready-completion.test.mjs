import {test} from 'node:test';
import assert from 'node:assert/strict';
import {advisoryPeriods} from './ready-completion.mjs';
test('principal period evidence uses allocation and actual debit, not demand or replies',()=>{
 const e=(stage,operation,id,code,ns=1)=>({stage,operation,id,code,ns});
 const periods=advisoryPeriods([e('reset','advisory-cycle',0,0),e('allocated','advisory-share',0,4),e('allocated','advisory-share',7,4),e('demand','advisory-share',7,0),e('debit','advisory-share',0,1),e('debit','advisory-share',0,1),e('reset','advisory-cycle',0,0,31e9),e('allocated','advisory-share',0,8),e('debit','advisory-share',0,1)]);
 assert.deepEqual(periods.map(p=>p.debits),[{0:2},{0:1}]);assert.deepEqual(periods.map(p=>p.allocations),[{0:4,7:4},{0:8}]);assert.equal(periods[0].demands.length,1);
});
import {reconcile} from './accounting.mjs';
test('opaque principal observations are not operation scopes in reconciliation',()=>{
 const events=[{seq:1,id:900,operation:'advisory-share',stage:'demand',code:0},{seq:2,id:900,operation:'advisory-share',stage:'allocated',code:8},{seq:3,id:900,operation:'advisory-share',stage:'debit',code:1}];
 assert.equal(reconcile(events,[]).unfinished.length,0);
});
