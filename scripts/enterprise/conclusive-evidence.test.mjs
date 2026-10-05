import assert from 'node:assert/strict';
import test from 'node:test';
import {verifyConclusive} from './conclusive-evidence.mjs';
function fixture(){
 const data={metadata:{commit:'controlled-verifier-fixture'},result:{pass:false,exit:{code:0},shutdown:{clean:true,telemetryLost:false},accounting:{unfinished:[],admittedRemaining:0},soak:{flatProgressFailure:{family:'stacks'},expectedNumbers:Array.from({length:145},(_,i)=>54+i),checkpoints:[{displayEvidence:{}}]}},native:[],ui:{measurement:{lost:false},telemetry:[],advisoryPublications:[]},provider:[]};
 for(const number of data.result.soak.expectedNumbers){
  const repo=`synthetic-lab/repo-${(number-1)%50+1}`,head=`head-${number}`,source={provider:'github',host:'github.com'};
  const push={repo,number,head_oid:head,base:'main',head_repo:repo,head_ref:`topic-${number}`};
  const stack={repo,number,head_oid:head,base_ref:'main',source};
  const at=number*100000,late=number===109,stackCall=late?8240:number;
  data.ui.telemetry.push({kind:'call',name:'get_ready_pushers',ok:true,rows:[push],at,duration:1,callId:1000+number,reply:[{...push,last_pusher:{state:'known'},rules:{state:'read'},pusher_valid_for_ms:60000,rules_valid_for_ms:600000}]},{kind:'call',name:'get_ready_stacks',ok:true,rows:[stack],at,duration:late?8:1,callId:stackCall,reply:[{...stack,stack:{kind:'none'},valid_for_ms:late?8:60000,last_known_stack:{age_ms:late?59991:0},advisory_progress:{outcome:'offered',admitted:true}}]});
  for(const id of [1000+number,stackCall])data.native.push({operation:'command',id,stage:'begin'},{operation:'command',id,stage:'complete'});
  const identity=JSON.stringify(['github','github.com',repo,number]);
  data.ui.advisoryPublications.push({number,kind:'ready-pushers',identity:[identity,push],owner:'synthetic-viewer',generation:1,at:at+1,freshPusher:true,freshRules:true},{number,kind:'ready-stack',identity:[identity,head,'main'],owner:'synthetic-viewer',generation:1,at:at+(late?8:1),freshStack:!late});
  for(const [i,stage] of ['pusher-activity','rules','stack-down','stack-up'].entries())data.provider.push({id:late&&i>=2?(i===2?952:967):2000+number*4+i,at:at+i,stage,status:200,terminal:'response-sent',subjects:[{repo,number}]});
 }
 data.result.accounting.submissions=data.result.accounting.providerReceipts=data.provider.length;return data;
}
test('all435 exact conclusive publications retain the original failed freshness verdict',()=>{const audit=verifyConclusive(fixture());assert.equal(audit.rawPass,false);assert.equal(audit.summary.stacks.conclusive,145);assert.deepEqual(audit.summary.stacks.retainedOnly,[109]);});
for(const [name,mutate] of Object.entries({
 duplicatePublication:d=>{d.ui.advisoryPublications.push({...d.ui.advisoryPublications[0]});},
 nativeRefused:d=>{d.native[1].stage='refused';},
 missing:d=>{d.ui.advisoryPublications=d.ui.advisoryPublications.filter(p=>p.number!==54);},
 owner:d=>{d.ui.advisoryPublications[0].owner='different';},
 generation:d=>{d.ui.advisoryPublications[0].generation=2;},
 head:d=>{d.ui.advisoryPublications[1].identity[1]='old-head';},
 unknown:d=>{d.ui.telemetry[1].reply[0].stack={kind:'unknown'};},
 inventedFresh:d=>{d.ui.advisoryPublications.find(p=>p.number===109&&p.kind==='ready-stack').freshStack=true;},
 beforeCompletion:d=>{d.ui.advisoryPublications[1].at-=100;},
 refusedProvider:d=>{d.provider.find(p=>p.stage==='stack-down'&&p.subjects[0].number===54).status=429;},
 changedPopulation:d=>{d.result.soak.expectedNumbers.pop();},
}))test(`rejects ${name} lineage`,()=>{const data=fixture();mutate(data);assert.throws(()=>verifyConclusive(data));});
