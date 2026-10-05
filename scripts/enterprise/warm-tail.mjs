import assert from 'node:assert/strict';
import {readFile,readdir} from 'node:fs/promises';
import {resolve,dirname} from 'node:path';
import {createHash} from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {mountedReadyIdentities,statsDOM,assertStatsMeasurement} from './final-acceptance.mjs';
const delay=ms=>new Promise(r=>setTimeout(r,ms));
export function replayWarmFixture(data,original,writes,actions){
 assert.deepEqual(original.soak.approved,['synthetic-lab/repo-1/51','synthetic-lab/repo-2/52','synthetic-lab/repo-3/53']);
 assert.equal(original.soak.obligations.externalClose,true);assert.equal(original.soak.obligations.historyMeasured,true);assert.equal(original.soak.obligations.mountedStats,true);
 assert.equal(original.shutdown.clean,true);assert.equal(original.shutdown.telemetryLost,false);assert.equal(original.exit.code,0);
 assert.equal(writes.length,3);assert.ok(writes.every(r=>r.operation==='review-write'&&r.status===200&&r.terminal==='response-sent'));
 assert.deepEqual(actions.map(a=>a.request.number).sort((a,b)=>a-b),[51,52,53]);
 for(const action of actions){
  assert.ok(action.role==='desktop'||action.role==='paired','verified trace role');
  const envelope=action.role==='desktop'?'value':'wire',other=action.role==='desktop'?'wire':'value';
  assert.ok(Object.hasOwn(action.response,envelope),'expected role-specific response envelope');assert.equal(Object.hasOwn(action.response,other),false,'reject ambiguous or wrong-role envelope');
  const value=action.response[envelope],ask=action.request,receipt=value?.receipt,row=data.rows.find(r=>r.number===ask.number);
  assert.equal(action.status,200);assert.equal(value?.outcome,'acknowledged');assert.ok(receipt,'acknowledged receipt');
  assert.equal(ask.expected_viewer,'synthetic-viewer');assert.equal(ask.verdict,'approve');assert.equal(ask.expected_head,row.headRefOid);assert.equal(ask.repo,row.repository.nameWithOwner);assert.equal(ask.id,row.id);
  assert.equal(receipt.number,row.number);assert.equal(receipt.repo,ask.repo);assert.equal(receipt.pr_id,row.id);assert.equal(receipt.actor,ask.expected_viewer);assert.equal(receipt.commit_oid,row.headRefOid);assert.equal(receipt.state,'APPROVED');
  const review={id:receipt.review_id,state:receipt.state,submittedAt:receipt.submitted_at,author:{login:receipt.actor},commit:{oid:receipt.commit_oid},pullRequest:row};
  row.reviews={...row.reviews,nodes:[review],totalCount:1};row.latestReviews={...row.latestReviews,nodes:[review],totalCount:1};
 }
 const last=original.soak.checkpoints.at(-1),source=Array.from({length:148},(_,i)=>{const n=i+51;return `synthetic-lab/repo-${(n-1)%50+1}/${n}`;}).sort();
 assert.deepEqual(last.inventory.desktop.reviewing,source);assert.deepEqual(last.inventory.paired.reviewing,source);
 const eligible=source.filter(id=>!original.soak.approved.includes(id));assert.deepEqual(last.inventory.displayed,eligible);
 const merged=data.rows.find(r=>r.number===199),closed=data.rows.find(r=>r.number===200);
 // Preserve the original close/merge day, not a new mutation receipt.
 const at=new Date(original.soak.checkpoints[0].stats.window.to+'T00:00:00Z');at.setUTCDate(at.getUTCDate()+1);
 for(const [row,state] of [[merged,'MERGED'],[closed,'CLOSED']]){row.state=state;row.closedAt=at.toISOString();row.updatedAt=row.closedAt;if(state==='MERGED')row.mergedAt=row.closedAt;}
 return {source,eligible,approved:original.soak.approved,actionLineage:actions.map(a=>({role:a.role,number:a.request.number,callId:a.response.callId,head:a.request.expected_head})),window:last.stats.window,meaning:'simulator replay of verified prior actions/closures; no current-session write evidence'};
}
export async function loadWarmReplay(inheritedProfile,copiedProfile,provider){
 assert.ok(inheritedProfile,'tail-warm requires an explicitly inherited profile');const dir=dirname(resolve(inheritedProfile));
 const bytes=await Promise.all(['result.json','provider.json','metadata.json'].map(n=>readFile(resolve(dir,n))));const [original,ledger,metadata]=bytes.map(b=>JSON.parse(b));
 const actions=[],hashes=Object.fromEntries(['result.json','provider.json','metadata.json'].map((n,i)=>[n,createHash('sha256').update(bytes[i]).digest('hex')]));
 for(const role of [0,1]){const archive=resolve(dir,`trace-${role}.zip`);hashes[`trace-${role}.zip`]=createHash('sha256').update(await readFile(archive)).digest('hex');const get=name=>execFileSync('unzip',['-p',archive,name],{maxBuffer:32*1024*1024});
  for(const line of get('trace.network').toString().trim().split('\n')){const s=JSON.parse(line).snapshot;if(s.request.url.endsWith('/review_pr_at_head'))actions.push({role:role===0?'desktop':'paired',request:JSON.parse(get(s.request.postData._file)).request,response:JSON.parse(get(s.response.content._file)),status:s.response.status});}
 }
 const state=replayWarmFixture(provider.data,original,ledger.filter(e=>e.operation==='review-write'),actions);
 const yesterday=new Date();yesterday.setUTCDate(yesterday.getUTCDate()-1);assert.equal(state.window.to,yesterday.toISOString().slice(0,10),'same closed-day question as persisted completed board');
 const profileHashes={};async function compare(path=''){for(const entry of await readdir(resolve(inheritedProfile,path),{withFileTypes:true})){const name=path?`${path}/${entry.name}`:entry.name;if(entry.isDirectory())await compare(name);else{const a=await readFile(resolve(inheritedProfile,name)),b=await readFile(resolve(copiedProfile,name));assert.ok(a.equals(b),'copied profile unchanged before native startup');profileHashes[name]=createHash('sha256').update(a).digest('hex');}}}await compare();
 return {...state,inheritedProfile:resolve(inheritedProfile),originalCommit:metadata.commit,inputSha256:hashes,profileSha256:profileHashes};
}
export async function runWarmTail({pages,provider,result,nativeCall,control}){
 const state=result.warmReplay;assert.ok(state);result.tail={scope:'new native/browser session; unchanged copied profile plus verified matching simulator state',startedAt:new Date().toISOString(),detailNumber:54,originalDetail52NotRepeated:true};
 await control('start');
 for(const page of pages){await page.getByTestId('counts').filter({hasText:'Authored 50 / Reviewing 148'}).waitFor({timeout:120000});await page.getByRole('heading',{name:/Ready for review \(145\)/}).waitFor();const inventory=await page.evaluate(()=>window.__enterprise.inventory());assert.deepEqual(inventory.reviewing,state.source);assert.equal(inventory.authored.length,50);assert.deepEqual(await mountedReadyIdentities(page),state.eligible);result.samples.push({role:page===pages[0]?'desktop':'paired'});}
 await pages[1].getByRole('button',{name:'Statistics',exact:true}).click();await pages[1].waitForFunction(()=>window.__enterprise.querySummary().some(q=>q.kind==='stats-board'&&q.observers>0&&q.complete&&q.total===250),null,{timeout:30000});
 const board=await pages[1].evaluate(()=>window.__enterprise.querySummary().find(q=>q.kind==='stats-board'&&q.observers>0));assertStatsMeasurement(board,await statsDOM(pages[1]),provider.data.rows.filter(r=>r.state==='MERGED'&&r.mergedAt.slice(0,10)>=board.window.from&&r.mergedAt.slice(0,10)<=board.window.to));
 const before=provider.ledger.length;result.tail.completeCacheReads=[];
 for(let i=0;i<5;i++){const response=await nativeCall('paired','stats_board',{scopeKind:'org',scopeValue:'synthetic-lab',measure:'merged',days:30});assert.equal(response.status,200);const reply=await response.json();assert.equal(reply.wire.complete,true);assert.equal(reply.wire.total,250);result.tail.completeCacheReads.push({callId:reply.callId,total:reply.wire.total,complete:reply.wire.complete});}
 assert.equal(provider.ledger.length,before);result.tail.cacheProviderBefore=before;result.tail.cacheProviderAfter=provider.ledger.length;
 await pages[1].getByRole('button',{name:'To Review',exact:true}).click();await pages[1].getByRole('button',{name:/Synthetic review 54(?:\D|$)/}).first().click();await pages[1].getByText('Synthetic description 54',{exact:false}).waitFor();await delay(3000);
 const count=()=>pages[1].evaluate(()=>window.__enterprise.telemetry.filter(e=>e.kind==='call'&&e.name==='get_pr_detail').length);const initial=await count(),started=performance.now();await delay(125000);assert.equal(await count(),initial);result.tail.unchangedDetail={number:54,elapsedMs:performance.now()-started,before:initial,after:await count()};
 await pages[1].getByRole('button',{name:'Back to list',exact:true}).click();result.tail.finalDOM=[];for(const page of pages){const rows=await mountedReadyIdentities(page);assert.deepEqual(rows,state.eligible);result.tail.finalDOM.push(rows);}
 assert.equal(provider.ledger.filter(e=>e.operation==='review-write').length,0);result.tail.newReviewWrites=0;result.tail.completedAt=new Date().toISOString();
}
