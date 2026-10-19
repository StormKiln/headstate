import {test} from 'node:test';
import assert from 'node:assert/strict';
import {expectedRevocationFailures} from './revocation-evidence.mjs';
function valid(){return {
 window:{start:100,end:200,confirmed:true},
 responses:[{role:'paired',command:'get_pr_detail',started:110,completed:150,status:502,error:'paired transport closed',callId:7}],
 native:[{id:7,operation:'command',stage:'begin',code:1,ns:10},{id:7,operation:'command',stage:'refused',code:0,ns:20}],
 failures:[{role:'paired',kind:'call',name:'get_pr_detail',callId:7,at:12,ok:false}],
 continuity:{before:true,during:true,after:true,repaired:true,errorCleared:true}
};}
test('intentional revocation requires exact response/native/call evidence and successful recovery',()=>{
 const v=valid();assert.deepEqual(expectedRevocationFailures(v),v.failures);
});
test('wrong response role command status reason identity or interval never becomes expected',()=>{
 for(const edit of [r=>r.role='desktop',r=>r.command='approve_pr',r=>r.status=500,r=>r.error='timeout',r=>r.callId=8,r=>r.started=99,r=>r.completed=201,r=>r.completed=109]){
  const v=valid();edit(v.responses[0]);assert.throws(()=>expectedRevocationFailures(v));
 }
});
test('native refusal must match the same paired command lifecycle',()=>{
 for(const edit of [v=>v.native=[],v=>v.native[0].code=0,v=>v.native[1].id=8,v=>v.native[1].stage='complete',v=>v.native[1].code=1,v=>v.native[1].ns=5,v=>v.native.push({...v.native[1]})]){
  const v=valid();edit(v);assert.throws(()=>expectedRevocationFailures(v));
 }
});
test('window and retention/recovery cannot be missing or unbounded',()=>{
 for(const edit of [v=>v.window.confirmed=false,v=>v.window.end=30101,v=>v.window.end=99,v=>v.window.start=NaN,...Object.keys(valid().continuity).map(k=>v=>v.continuity[k]=false)]){
  const v=valid();edit(v);assert.throws(()=>expectedRevocationFailures(v));
 }
});
test('unrelated failed calls remain outside the expected set even with matching timestamps',()=>{
 const v=valid();v.failures.push({...v.failures[0],role:'desktop'},{...v.failures[0],callId:8},{...v.failures[0],name:'approve_pr'});
 assert.deepEqual(expectedRevocationFailures(v),[v.failures[0]]);
});
test('actual Refresh fan-out correlates detail gates and ancestry independently',()=>{
 const v=valid();
 for(const [index,name] of ['get_review_gates','get_ready_stacks'].entries()){
  const callId=8+index;v.responses.push({...v.responses[0],command:name,callId});v.failures.push({...v.failures[0],name,callId});v.native.push(...v.native.slice(0,2).map(row=>({...row,id:callId})));
 }
 assert.deepEqual(expectedRevocationFailures(v),v.failures);
 for(const edit of [r=>r.command='get_pr_detail',r=>r.error='provider timeout',r=>r.role='desktop',r=>r.started=99,r=>r.command='get_viewer',r=>r.command='approve_pr']){
  const copy=structuredClone(v);edit(copy.responses[1]);assert.deepEqual(expectedRevocationFailures(copy),[copy.failures[0],copy.failures[2]]);
 }
 const noDetail=structuredClone(v);noDetail.failures=noDetail.failures.slice(1);assert.throws(()=>expectedRevocationFailures(noDetail),'gate/ancestry refusals cannot replace deliberate detail proof');
});
