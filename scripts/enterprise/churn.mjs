// Read only: callers supply existing UI witnesses; this never refreshes a provider.
export async function observeUntil({deadline,observe,now=()=>performance.now(),sleep=ms=>new Promise(resolve=>setTimeout(resolve,ms))}){
 for(;;){await observe();const remaining=deadline-now();if(remaining<=0)return;await sleep(Math.min(1000,remaining));}
}
export const fatalObservation=error=>/regress|bounded|lost|continuity/.test(error.message);
export async function observeRoles(clients,observe){
 const results=await Promise.allSettled(clients.map(observe));
 const errors=results.filter(result=>result.status==='rejected').map(result=>result.reason);
 if(errors.length)throw errors.find(fatalObservation)??errors[0];
}
import assert from 'node:assert/strict';import {writeFile,open} from 'node:fs/promises';import {resolve} from 'node:path';
import {createChurnPlan,initialOracle,advanceOracle,assertChurnFacts,assertAcceptedFacts,assertChurnReady,expectedChurnReady,assertChurnSchedule,churnDeadline,assertHealthyChurnPhases,observeChurnProgress,digest} from './churn-plan.mjs';import {applyChurnEvent} from './provider.mjs';
const delay=ms=>new Promise(r=>setTimeout(r,ms));
const snapshot=page=>page.evaluate(()=>({facts:window.__enterprise.rowFacts(),ready:window.__enterprise.readyEligibility(),source:window.__enterprise.sourceEvidence(),lost:window.__enterprise.measurement.lost,accepted:window.__enterprise.telemetry.filter(e=>e.kind==='event'&&e.name==='source-poll-status'&&e.reply?.list==='reviewing'&&e.reply?.prs&&e.reply?.last_received_at===window.__enterprise.sourceEvidence()?.lastReceivedAt).at(-1)?.reply}));
async function navigate(page,number){const back=page.getByRole('button',{name:'Back to list',exact:true});if(await back.count())await back.click();await page.getByRole('button',{name:'To Review',exact:true}).click();await page.getByRole('button',{name:new RegExp(`Synthetic review ${number}(?:\\D|$)`)}).first().click();await page.getByText(`Synthetic description ${number}`,{exact:false}).waitFor();}
async function mountContinuity(page){await navigate(page,53);await page.getByText('Open report',{exact:true}).click();const input=page.getByPlaceholder('Reply…').first();await input.fill('Retained churn backward draft');await input.evaluate(node=>{node.focus();node.setSelectionRange(3,17,'backward');window.__continuityNode=node;});}
async function assertContinuity(page){const observed=await page.evaluate(()=>{const n=window.__continuityNode;return {connected:n?.isConnected,focused:document.activeElement===n,value:n?.value,start:n?.selectionStart,end:n?.selectionEnd,direction:n?.selectionDirection,open:[...document.querySelectorAll('details')].some(d=>d.open&&d.querySelector('summary')?.textContent==='Open report')};});assert.deepEqual(observed,{connected:true,focused:true,value:'Retained churn backward draft',start:3,end:17,direction:'backward',open:true});return observed;}
export async function runChurn({pages,provider,result,out,control,workload,state}){
 const plan=state?.plan??createChurnPlan(workload);let oracle=state?.oracle??initialOracle(plan);const receipts=state?.receipts??[];
 const activeStarted=performance.now();
 const evidence={schema:1,readyCoverage:'all-checkpoints-in-this-segment',sourceReadyCoverage:state?.sourceReadyCoverage??null,integrationOnly:process.env.ENTERPRISE_CHURN_INTEGRATION_ONLY==='1',requiredActiveMs:1800000,activeSegmentsMs:state?.activeSegmentsMs??[],sustainedPass:false,planHash:digest(plan),startRevision:oracle.revision,segmentPass:false,planComplete:false,events:[],checkpoints:[],policy:{...plan.policy,maxDocumentsPerEvent:500,maxDocumentsPerSegment:7000}};result.churn=evidence;result.samples.push({role:'desktop'},{role:'paired'});
 const save=()=>writeFile(resolve(out,'churn.json'),JSON.stringify(evidence,null,2));await writeFile(resolve(out,'churn-plan.json'),JSON.stringify({plan,hash:digest(plan)},null,2));
 const acceptedByRole=new Map(),readNative=createNativeReader(resolve(out,'profile/native.ndjson'));let continuity=false,activeEvent,nativeState;
 const observe=async()=>{try{const checked=assertHealthyChurnPhases(await readNative(),nativeState);nativeState=checked.state;evidence.nativePhases={maxMs:checked.maxMs,phases:checked.phases};}catch(error){throw Error(`native phase evidence lost: ${error.message}`);}const clients=await Promise.all(pages.map(snapshot));if(activeEvent)activeEvent.record.progress=observeChurnProgress(activeEvent.record.progress,{event:activeEvent.event,ledger:provider.ledger.slice(activeEvent.record.providerBefore),clients,now:performance.now()});await observeRoles(clients,async(client,i)=>{if(continuity){try{await assertContinuity(pages[i]);}catch(error){throw Error(`continuity lost: ${error.message}`);}}assert.equal(client.lost,false);const r=client.accepted,previous=acceptedByRole.get(i);if(r){let converged=false;try{assertChurnFacts(oracle,client.facts);assertChurnReady(oracle,client.ready);converged=true;}catch{}const accepted={session:r.session,revision:r.receipt_revision,facts:client.facts,oracleRevision:oracle.revision,converged};assertAcceptedFacts(previous,accepted);acceptedByRole.set(i,accepted);}assertChurnFacts(oracle,client.facts);assertChurnReady(oracle,client.ready);});return clients;};
 const converge=async(event)=>{const began=performance.now(),deadline=event?churnDeadline(event.kind,plan.policy):plan.policy.convergenceMs;let last;while(performance.now()-began<deadline){try{const clients=await observe();evidence.checkpoints.push({revision:oracle.revision,expectedReady:expectedChurnReady(oracle),elapsedMs:performance.now()-began,clients});await save();return;}catch(error){if(fatalObservation(error))throw error;last=error;}await delay(1000);}throw Error(`Churn revision ${oracle.revision} did not converge: ${last?.message}`);};
 try{
  if(state)await converge();await control('start');await converge();
  const started=performance.now(),baseOffset=plan.events[oracle.revision]?.nominalOffsetMs??0;
  const stop=state?plan.events.length:plan.checkpointRevision;
  for(const event of plan.events.slice(oracle.revision,stop)){
   const nominal=event.nominalOffsetMs-baseOffset;const previous=evidence.events.at(-1);const scheduled=Math.max(nominal,previous?previous.actualOffsetMs+plan.policy.spacingMs:0);await observeUntil({deadline:started+scheduled,observe});
   const before=provider.ledger.length,actualOffsetMs=performance.now()-started;assert.ok(actualOffsetMs<plan.events.slice(evidence.startRevision,stop).reduce((sum,event)=>sum+plan.policy.spacingMs+churnDeadline(event.kind,plan.policy),0),'finite segment schedule bound');
   const record={revision:event.revision,kind:event.kind,identity:event.identity,nominalOffsetMs:event.nominalOffsetMs,actualOffsetMs,nominalDriftMs:actualOffsetMs-nominal,providerBefore:before,deadlineMs:churnDeadline(event.kind,plan.policy)};evidence.events.push(record);activeEvent={event,record};await save();
   let receipt;
   if(event.kind==='navigate')await navigate(pages[event.role==='desktop'?0:1],event.next.number);
   else if(event.kind==='approve'){
    const page=pages[event.role==='desktop'?0:1];await page.getByRole('button',{name:'Approve',exact:true}).first().click();await page.getByRole('button',{name:'Approved',exact:true}).first().waitFor({timeout:30000});
    const writes=provider.ledger.slice(before).filter(e=>e.operation==='review-write'&&e.status===200&&e.terminal==='response-sent'&&e.responseDelivered);assert.equal(writes.length,1,'one confirmed real review mutation');receipt=writes[0].mutationReceipt;record.mutation=structuredClone(writes[0]);receipts.push({revision:event.revision,receipt});
   }else record.adapter=applyChurnEvent(provider.data,event);
   oracle=advanceOracle(oracle,event,receipt);await converge(event);activeEvent=undefined;record.providerAfter=provider.ledger.length;record.documents=record.providerAfter-before;assert.ok(record.documents<=evidence.policy.maxDocumentsPerEvent,'bounded documents per event');record.convergedOffsetMs=performance.now()-started;assertChurnSchedule(evidence.events,plan.events.slice(evidence.startRevision,oracle.revision).map(event=>event.revision),plan.policy);
   if(event.revision===4||state&&!continuity){for(const page of pages)await mountContinuity(page);continuity=true;record.retentionMounted=true;}
   await save();console.log(JSON.stringify({churnRevision:oracle.revision,kind:event.kind,providerReceipts:provider.ledger.length}));
  }
  assertChurnSchedule(evidence.events,plan.events.slice(evidence.startRevision,stop).map(event=>event.revision),plan.policy);
  if(state&&!evidence.integrationOnly){while(evidence.activeSegmentsMs.reduce((a,b)=>a+b,0)+performance.now()-activeStarted<evidence.requiredActiveMs){await observeUntil({deadline:performance.now()+5000,observe});}}
  assert.ok(provider.ledger.length<=evidence.policy.maxDocumentsPerSegment,'bounded documents per segment');
  evidence.activeSegmentsMs=[...evidence.activeSegmentsMs,performance.now()-activeStarted];
  evidence.segmentPass=true;evidence.planComplete=oracle.revision===plan.events.length;evidence.endRevision=oracle.revision;evidence.remaining=plan.events.slice(oracle.revision).map(e=>e.revision);evidence.sustainedPass=evidence.planComplete&&!evidence.integrationOnly&&evidence.sourceReadyCoverage==='all-checkpoints-in-this-segment'&&evidence.activeSegmentsMs.reduce((a,b)=>a+b,0)>=evidence.requiredActiveMs;return {plan,oracle,receipts,activeSegmentsMs:evidence.activeSegmentsMs};
 }finally{evidence.oracle=oracle;await save();}
}

export function createNativeReader(path){
 let offset=0,carry='',identity,nextSequence=1;const pending=new Map();
 return async()=>{const file=await open(path,'r');try{const stat=await file.stat();const current=`${stat.dev}:${stat.ino}`;assert.ok(!identity||identity===current,'native evidence replaced');identity=current;assert.ok(stat.size>=offset,'native evidence truncated');const events=[];
  while(offset<stat.size){const bytes=Buffer.alloc(Math.min(131072,stat.size-offset));const {bytesRead}=await file.read(bytes,0,bytes.length,offset);assert.ok(bytesRead>0,'native evidence short read');offset+=bytesRead;carry+=bytes.toString('utf8',0,bytesRead);const last=carry.lastIndexOf('\n');if(last>=0){events.push(...carry.slice(0,last).split('\n').filter(Boolean).map(JSON.parse));carry=carry.slice(last+1);}assert.ok(carry.length<=65536,'native line evidence bounded');}
  for(const event of events){assert.ok(Number.isSafeInteger(event.seq)&&event.seq>=nextSequence&&!pending.has(event.seq),'native duplicate/invalid sequence');pending.set(event.seq,event);}const ordered=[];while(pending.has(nextSequence)){ordered.push(pending.get(nextSequence));pending.delete(nextSequence++);}assert.ok(pending.size<=1024,'native sequence gap bounded');return ordered;
 }finally{await file.close();}};
}
