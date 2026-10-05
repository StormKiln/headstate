import {test} from 'node:test';import assert from 'node:assert/strict';import {fixture,manifest,searchRows} from './provider.mjs';
test('declared enterprise dimensions and disjoint memberships are facts',()=>{const d=fixture();assert.equal(d.rows.filter(r=>r.state==='OPEN').length,manifest.openPrs);assert.equal(new Set(d.rows.map(r=>r.id)).size,manifest.openPrs+manifest.historicalMerged);assert.equal(new Set(d.rows.map(r=>r.repository.nameWithOwner)).size,50);assert.equal(new Set(d.rows.map(r=>r.author.login)).size,50);const authored=searchRows(d.rows,'is:pr is:open author:@me');const reviewing=searchRows(d.rows,'is:pr is:open review-requested:@me');assert.equal(authored.length,50);assert.equal(reviewing.length,150);assert.equal(authored.filter(a=>reviewing.includes(a)).length,0);assert.equal(searchRows(d.rows,'is:pr is:open repo:synthetic-lab/repo-1').length,4);assert.equal(searchRows(d.rows,'is:pr is:merged org:synthetic-lab').length,250);assert.throws(()=>searchRows(d.rows,'unrecognized:filter'));});
import {startProvider} from './provider.mjs';
async function usingProvider(run){const p=await startProvider();try{await run(p,async query=>{const r=await fetch(p.url+'/graphql',{method:'POST',body:JSON.stringify({query}),headers:{'content-type':'application/json'}});assert.equal(r.status,200);return (await r.json()).data;});}finally{await p.close();}}
test('unknown organization, repository owner/name, and wrong PR repository do not return fallback facts',()=>usingProvider(async(_p,query)=>{const d=await query('{ organization(login:"absent"){login} repository(owner:"absent",name:"repo-1"){nameWithOwner} wrong:repository(owner:"synthetic-lab",name:"repo-2"){pullRequest(number:51){id}} }');assert.equal(d.organization,null);assert.equal(d.repository,null);assert.equal(d.wrong.pullRequest,null);}));
test('close transition agrees across search and direct identity reads',()=>usingProvider(async(p,query)=>{p.fault.closed=[51];const d=await query('{ search(query:"is:pr is:open repo:synthetic-lab/repo-1",first:100){nodes{id}} node(id:"PR_51"){id state closedAt} }');assert.equal(d.search.nodes.some(n=>n.id==='PR_51'),false);assert.equal(d.node.state,'CLOSED');assert.ok(d.node.closedAt);}));
test('review receipt preserves explicitly requested commit instead of inventing current-head confirmation',()=>usingProvider(async(_p,query)=>{const d=await query('mutation {addPullRequestReview(input:{pullRequestId:"PR_51",event:APPROVE,commitOID:"stale-head"}){pullRequestReview{commit{oid} pullRequest{headRefOid}}}}');assert.equal(d.addPullRequestReview.pullRequestReview.commit.oid,'stale-head');assert.equal(d.addPullRequestReview.pullRequestReview.pullRequest.headRefOid,'head-51');}));
test('activity facts match requested repository branch and current head',()=>usingProvider(async(p)=>{const response=await fetch(p.url+'/repos/synthetic-lab/repo-1/activity?ref=refs%2Fheads%2Ftopic-51&per_page=10');assert.equal(response.status,200);const [activity]=await response.json();assert.equal(activity.after,'head-51');assert.equal(activity.actor.login,p.data.rows[50].author.login);const absent=await fetch(p.url+'/repos/absent/repo-1/activity');assert.equal(absent.status,404);}));

test('historical PR head and commit agree and occupy a recent closed day',()=>{for(const row of fixture().rows.filter(r=>r.state==='MERGED')){assert.equal(row.commits.nodes[0].commit.oid,row.headRefOid);assert.equal(row.mergedAt.slice(0,10),manifest.historyClosedDay);assert.ok(Date.now()-Date.parse(row.mergedAt)>0);assert.ok(Date.now()-Date.parse(row.mergedAt)<2*86400000);}});

test('synthetic advisory ledger attributes actual production-shaped downward/upward/activity identities',()=>usingProvider(async(p,query)=>{
 await query('query PrStack {repository(owner:"synthetic-lab",name:"repo-1"){pullRequest(number:51){number headRefOid}}}');
 await query('query PrStackUp {repository(owner:"synthetic-lab",name:"repo-1"){pullRequests(baseRefName:"topic-51",first:10){nodes{number}}}}');
 await fetch(p.url+'/repos/synthetic-lab/repo-1/activity?ref=refs%2Fheads%2Ftopic-51');
 assert.deepEqual(p.ledger.map(e=>({stage:e.stage,subjects:e.subjects})),[
  {stage:'stack-down',subjects:[{repo:'synthetic-lab/repo-1',number:51}]},
  {stage:'stack-up',subjects:[{repo:'synthetic-lab/repo-1',number:51}]},
  {stage:'pusher-activity',subjects:[{repo:'synthetic-lab/repo-1',number:51}]},
 ]);
}));

test('opt-in convergence fixture declares independent 236 reviewing and 130 Ready identities',()=>{
 const data=fixture({convergence:true});
 const reviewing=searchRows(data.rows,'is:pr is:open review-requested:@me');
 assert.deepEqual(reviewing.map(row=>row.number),Array.from({length:236},(_,i)=>51+i));
 assert.deepEqual(reviewing.filter(row=>!row.isDraft).map(row=>row.number),Array.from({length:130},(_,i)=>51+i));
 assert.equal(searchRows(data.rows,'is:pr is:open author:@me').length,50);
 assert.ok(new Set(reviewing.map(row=>row.repository.nameWithOwner)).size>=50);
 assert.ok(data.members.length>=50);
 assert.equal(fixture().rows.filter(row=>row.state==='OPEN').length,200,'ordinary fixture remains unchanged');
});

test('targeted held search materializes stale values while detail and mutations continue',async()=>{
 const p=await startProvider({convergence:true});
 const query=async q=>(await (await fetch(p.url+'/graphql',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({query:q})})).json()).data;
 try{
  p.fault.holdSearch={list:'reviewing',after:null,remaining:1};
  const held=query('{search(query:"is:pr is:open review-requested:@me",first:25){nodes{id isDraft}}}');
  for(let i=0;i<100&&!p.heldCount;i++)await new Promise(r=>setTimeout(r,5));
  assert.equal(p.heldCount,1);
  const mutation=await query('mutation {convertPullRequestToDraft(input:{pullRequestId:"PR_51"}){pullRequest{id isDraft updatedAt}}}');
  assert.equal(mutation.convertPullRequestToDraft.pullRequest.isDraft,true);
  const fresh=await query('{node(id:"PR_51"){id isDraft}}');assert.equal(fresh.node.isDraft,true);
  p.release();
  assert.equal((await held).search.nodes.find(row=>row.id==='PR_51').isDraft,false,'held reply must preserve pre-mutation provider facts');
 }finally{p.release();await p.close();}
});

test('one-shot failed continuation leaves head and detail queries available',async()=>{
 const p=await startProvider({convergence:true});
 const query=q=>fetch(p.url+'/graphql',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({query:q})});
 try{
  p.fault.failSearch={list:'reviewing',after:'cursor-50',remaining:1};
  const q='{search(query:"is:pr is:open review-requested:@me",first:25,after:"cursor-50"){nodes{id}}}';
  assert.equal((await query('{node(id:"PR_51"){id}}')).status,200);
  assert.equal((await query(q)).status,503);
  assert.equal((await query(q)).status,200);
  assert.equal(p.ledger.filter(e=>e.targetedFailure).length,1);
 }finally{p.release();await p.close();}
});

test('dequeue consumes schema id and rejects enqueue-style pullRequestId',async()=>{
 const p=await startProvider({convergence:true});const query=q=>fetch(p.url+'/graphql',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({query:q})});
 try{
  const invalid=await query('mutation {dequeuePullRequest(input:{pullRequestId:"PR_51"}){mergeQueueEntry{state}}}');assert.equal(invalid.status,500);
  const response=await query('mutation {dequeuePullRequest(input:{id:"PR_51"}){mergeQueueEntry{state pullRequest{id isInMergeQueue mergeQueueEntry{state} updatedAt}}}}');assert.equal(response.status,200);
  const receipt=(await response.json()).data.dequeuePullRequest;assert.equal(receipt.mergeQueueEntry.pullRequest.id,'PR_51');assert.equal(receipt.mergeQueueEntry.pullRequest.isInMergeQueue,false);assert.equal(receipt.mergeQueueEntry.pullRequest.mergeQueueEntry,null);
 }finally{await p.close();}
});

test('ledger distinguishes primary detail identity from check continuations and stack reads',()=>usingProvider(async(p,query)=>{
 await query('{repository(owner:"synthetic-lab",name:"repo-1"){pullRequest(number:51){id body reviews(first:1){nodes{id}} reviewThreads(first:1){nodes{id}}}}}');
 await query('{repository(owner:"synthetic-lab",name:"repo-1"){pullRequest(number:51){id reviews(first:1){nodes{id}}}}}');
 assert.deepEqual(p.ledger[0].primaryDetails,[{repo:'synthetic-lab/repo-1',number:51}]);assert.deepEqual(p.ledger[1].primaryDetails,[]);
}));
