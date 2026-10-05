import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import {createHash} from 'node:crypto';
import {pathToFileURL} from 'node:url';

// An additional offline qualification. Never rewrites the original soak verdict.
export function verifyConclusive({result,ui,provider,metadata,native}){
 const expected=Array.from({length:145},(_,i)=>54+i);
 assert.equal(result.pass,false);assert.equal(result.soak.flatProgressFailure.family,'stacks');
 assert.deepEqual(result.soak.expectedNumbers,expected);assert.equal(ui.measurement.lost,false);
 assert.equal(result.shutdown.clean,true);assert.equal(result.shutdown.telemetryLost,false);assert.equal(result.exit.code,0);
 assert.deepEqual(result.accounting.unfinished,[]);assert.equal(result.accounting.admittedRemaining,0);
 assert.equal(result.accounting.submissions,provider.length);assert.equal(result.accounting.providerReceipts,provider.length);
 const witnesses={pushers:[],rules:[],stacks:[]};
 const commandScopes=new Map();for(const event of native.filter(e=>e.operation==='command')){const scope=commandScopes.get(event.id)??[];scope.push(event);commandScopes.set(event.id,scope);}
 for(const number of expected){
  const repo=`synthetic-lab/repo-${((number-1)%50)+1}`,head=`head-${number}`;
  for(const family of Object.keys(witnesses)){
   const stack=family==='stacks',kind=stack?'ready-stack':'ready-pushers',command=stack?'get_ready_stacks':'get_ready_pushers';
   const field={pushers:'freshPusher',rules:'freshRules',stacks:'freshStack'}[family];
   const ttlField={pushers:'pusher_valid_for_ms',rules:'rules_valid_for_ms',stacks:'valid_for_ms'}[family];
   const maximum=family==='rules'?600000:60000;
   const candidates=[];
   for(const call of ui.telemetry.filter(e=>e.kind==='call'&&e.name===command&&e.ok)){
    const ask=call.rows?.find(r=>r.number===number&&r.repo===repo&&r.head_oid===head&&(stack?r.base_ref:r.base)==='main');if(!ask)continue;
    if(stack)assert.deepEqual(ask.source,{provider:'github',host:'github.com'});
    else {assert.equal(ask.head_repo,repo);assert.equal(ask.head_ref,`topic-${number}`);}
    const row=call.reply?.find(r=>r.number===number&&r.repo===repo&&r.head_oid===head&&(stack?r.base_ref:r.base)==='main');if(!row)continue;
    const conclusive=stack?row.stack?.kind==='none':family==='pushers'?row.last_pusher?.state==='known':row.rules?.state==='read';
    if(!conclusive)continue;
    if(stack)assert.deepEqual(row.source,ask.source);
    else {assert.equal(row.head_repo,repo);assert.equal(row.head_ref,ask.head_ref);}
    const ttl=row[ttlField];assert.ok(Number.isFinite(ttl)&&ttl>=0&&ttl<=maximum);
    const publications=ui.advisoryPublications.filter(p=>p.kind===kind&&p.number===number&&p.at>=call.at+call.duration&&p.at<=call.at+call.duration+5);
    assert.equal(publications.length,1,'one unique same-identity production success per conclusive command');
    const scope=commandScopes.get(call.callId);assert.deepEqual(scope?.map(e=>e.stage),['begin','complete'],'actual native command completed');
    for(const publication of publications){
     assert.equal(publication.owner,'synthetic-viewer');assert.equal(publication.generation,1);
     assert.deepEqual(JSON.parse(publication.identity[0]),['github','github.com',repo,number]);
     if(stack)assert.deepEqual(publication.identity.slice(1),[head,'main']);else assert.deepEqual(publication.identity[1],ask);
     const expiry=call.at+ttl,fresh=expiry>publication.at;
     assert.equal(publication[field],fresh,'actual success publication freshness agrees with original call-start lifetime');
     const stages=stack?['stack-down','stack-up']:family==='pushers'?['pusher-activity']:['rules'];
     const receipts=stages.map(stage=>provider.filter(e=>e.stage===stage&&e.status===200&&e.terminal==='response-sent'&&e.subjects?.some(s=>s.repo===repo&&(stage==='rules'||s.number===number))).map(e=>e.id));
     assert.ok(receipts.every(ids=>ids.length),'each conclusive family has actual successful provider stage evidence');
     candidates.push({number,repo,head,base:'main',owner:publication.owner,generation:publication.generation,callId:call.callId,callStarted:call.at,duration:call.duration,originalValidForMs:ttl,expiresAt:expiry,publicationAt:publication.at,freshAtPublication:fresh,progress:row.advisory_progress,receipt:row,corroboratingProviderStages:Object.fromEntries(stages.map((stage,i)=>[stage,receipts[i]]))});
    }
   }
   assert.ok(candidates.length,`missing exact conclusive ${family} publication for${number}`);
   const chosen=candidates.find(c=>c.freshAtPublication)??candidates[0];
   witnesses[family].push({...chosen,firstConclusivePublicationAt:Math.min(...candidates.map(c=>c.publicationAt)),lastConclusivePublicationAt:Math.max(...candidates.map(c=>c.publicationAt))});
  }
 }
 const summary=Object.fromEntries(Object.entries(witnesses).map(([family,rows])=>[family,{conclusive:rows.length,everFresh:rows.filter(r=>r.freshAtPublication).length,retainedOnly:rows.filter(r=>!r.freshAtPublication).map(r=>r.number),unknown:[]}]));
 assert.deepEqual(summary.stacks.retainedOnly,[109]);assert.equal(summary.pushers.everFresh,145);assert.equal(summary.rules.everFresh,145);
 const boundary=witnesses.stacks.find(r=>r.number===109);assert.equal(boundary.callId,8240);assert.equal(boundary.originalValidForMs,8);assert.equal(boundary.duration,8);assert.equal(boundary.expiresAt,boundary.publicationAt);assert.deepEqual(boundary.progress,{outcome:'offered',admitted:true});assert.equal(boundary.receipt.last_known_stack.age_ms,59991);
 const down=provider.find(r=>r.id===952),up=provider.find(r=>r.id===967);assert.ok(boundary.corroboratingProviderStages['stack-down'].includes(952)&&boundary.corroboratingProviderStages['stack-up'].includes(967));assert.ok(up.at>down.at&&up.at-down.at<60000);
 return {qualification:'separate ever-conclusive audit; original freshness soak remains FAIL',providerStageQualification:'same-identity successful stages corroborate parsed native conclusions; not per-command HTTP attribution (cache reuse allowed)',source:metadata,rawPass:result.pass,summary,witnesses,boundary109:{down,up,publication:boundary},finalDisplay:result.soak.checkpoints.at(-1).displayEvidence};
}
if(process.argv[1]&&import.meta.url===pathToFileURL(resolve(process.argv[1])).href){
 const directory=resolve(process.argv[2]);const read=async name=>JSON.parse(await readFile(resolve(directory,name),'utf8'));
 const [result,ui,provider,metadata]=await Promise.all(['result.json','ui-0.json','provider.json','metadata.json'].map(read));
 const native=(await readFile(resolve(directory,'native.ndjson'),'utf8')).trim().split('\n').map(JSON.parse);
 const audit=verifyConclusive({result,ui,provider,metadata,native});audit.inputSha256=Object.fromEntries(await Promise.all(['result.json','ui-0.json','provider.json','metadata.json','native.ndjson'].map(async name=>[name,createHash('sha256').update(await readFile(resolve(directory,name))).digest('hex')])));await writeFile(resolve(directory,'conclusive-audit.json'),JSON.stringify(audit,null,2));console.log(JSON.stringify(audit.summary));
}
