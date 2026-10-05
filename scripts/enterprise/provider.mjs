import {createServer} from 'node:http';import {parseDocument,project} from './graphql.mjs';
export const manifest={schema:1,seed:9001,repositories:50,members:50,openPrs:200,authored:50,reviewing:150,overlap:0,historicalMerged:250};
const date=new Date(Date.now()-86400000).toISOString().slice(0,10)+'T12:00:00Z';
manifest.historyClosedDay=date.slice(0,10);
const connection=nodes=>({nodes,totalCount:nodes.length,issueCount:nodes.length,pageInfo:{hasNextPage:false,endCursor:null}});
export function fixture() {
 const members=Array.from({length:50},(_,i)=>({login:i===0?"synthetic-viewer":`synthetic-member-${i+1}`,name:`Synthetic member ${i+1}`,avatarUrl:''}));
 const repos=Array.from({length:50},(_,i)=>({name:`repo-${i+1}`,nameWithOwner:`synthetic-lab/repo-${i+1}`,owner:{login:'synthetic-lab'},defaultBranchRef:{name:'main'},url:`https://github.com/synthetic-lab/repo-${i+1}`,isArchived:false,isPrivate:true,pushedAt:date}));
 const rows=Array.from({length:200},(_,i)=>{const number=i+1,repo=repos[i%50];return {
  id:`PR_${number}`,number,title:`Synthetic review ${number}`,url:`https://github.com/${repo.nameWithOwner}/pull/${number}`,state:'OPEN',isDraft:false,createdAt:date,updatedAt:date,readyAt:date,
  body:`Synthetic description ${number}\n<details><summary>Open report</summary>Retained disclosure</details>`,author:i<50?members[0]:members[1+(i-50)%49],repository:repo,
  headRefName:`topic-${number}`,headRefOid:`head-${number}`,headRef:{id:`REF_${number}`},headRepository:repo,baseRefName:'main',
  mergeable:'MERGEABLE',mergeStateStatus:'CLEAN',reviewDecision:null,isInMergeQueue:false,isMergeQueueEnabled:false,mergeQueueEntry:null,mergeQueue:null,
  additions:5,deletions:1,changedFiles:1,totalCommentsCount:1,
  labels:connection([]),assignees:connection([]),reviewRequests:connection(i<50?[]:[{requestedReviewer:{login:"synthetic-viewer",__typename:"User"}}]),latestReviews:connection([]),reviews:connection([]),
  comments:connection([{id:`C_${number}`,author:{login:'synthetic-member-50',__typename:'User'},createdAt:date,body:'Synthetic discussion'}]),
  reviewThreads:connection([1,2].map(t=>({id:`T_${number}_${t}`,isResolved:false,isOutdated:false,path:'synthetic.rs',line:t,viewerCanReply:true,viewerCanResolve:true,viewerCanUnresolve:true,comments:connection([{id:`TC_${number}_${t}`,author:{login:'synthetic-member-2',__typename:'User'},createdAt:date,body:`Synthetic thread ${t}`}])}))),
  timelineItems:connection([{__typename:'ReadyForReviewEvent',createdAt:date}]),
  commits:connection([{commit:{oid:`head-${number}`,statusCheckRollup:{state:'SUCCESS',contexts:connection([{__typename:'CheckRun',id:`CHECK_${number}`,name:'Synthetic check',status:'COMPLETED',conclusion:'SUCCESS',detailsUrl:null,checkSuite:{databaseId:1}}])}}}]),
 };});
 const history=Array.from({length:manifest.historicalMerged},(_,i)=>{const row=structuredClone(rows[i%rows.length]);const number=1001+i;row.commits.nodes[0].commit.oid=`head-${number}`;return {...row,id:`PR_${number}`,number,title:`Synthetic historical review ${number}`,url:`https://github.com/${row.repository.nameWithOwner}/pull/${number}`,state:'MERGED',createdAt:new Date(Date.parse(date)-2*86400000).toISOString(),mergedAt:date,closedAt:date,updatedAt:date,headRefName:`topic-${number}`,headRefOid:`head-${number}`,reviewRequests:connection([])};});
 return {members,repos,rows:[...rows,...history]};
}
export async function startProvider() {
 const held=[];const data=fixture(),ledger=[],fault={mode:'healthy',delay:0,thread:false,outdated:false,closed:[]};let active=0,next=0,lost=false;const quota=Object.fromEntries(['graphql','rest'].map(bucket=>[bucket,{remaining:5000,reset:Math.floor(Date.now()/1000)+120}]));
 const server=createServer(async(req,res)=>{
  const id=++next,started=performance.now();active++;let entry={id,at:started,operation:'unknown',fault:fault.mode,aliases:0,status:0,active,terminal:null};if(ledger.length<100000)ledger.push(entry);else lost=true;
  let raw='';for await(const c of req)raw+=c;
  const bucket=req.url==='/graphql'?'graphql':'rest';
  const window=quota[bucket];if(Math.floor(Date.now()/1000)>=window.reset){window.remaining=5000;window.reset=Math.floor(Date.now()/1000)+120;}
  window.remaining=Math.max(0,window.remaining-1);entry.bucket=bucket;entry.remaining=window.remaining;entry.reset=window.reset;
  res.setHeader('x-ratelimit-remaining',String(entry.remaining));res.setHeader('x-ratelimit-reset',String(entry.reset));
  try {
   for(const row of data.rows){const state=fault.merged?.includes(row.number)?'MERGED':fault.closed.includes(row.number)?'CLOSED':null;if(state&&row.state!==state){row.state=state;row.closedAt=new Date().toISOString();row.updatedAt=row.closedAt;if(state==='MERGED')row.mergedAt=row.closedAt;}}
   const responseDelay=fault.delays?.shift()??fault.delay;
   const input=req.url==='/graphql'?JSON.parse(raw):null,doc=input?parseDocument(input.query,input.variables):null;
   if(doc){entry.aliases=doc.aliases;const names=doc.selection.map(f=>f.name);entry.operation=doc.kind==='mutation'?'review-write':names.includes('search')?'search':names.includes('node')||names.includes('nodes')?'node':names.includes('repository')?'repository':names.includes('viewer')||names.includes('organization')?'viewer':'unknown';}
   else entry.operation=req.method==='GET'?'rest-read':'rest-write';
   const historyRead=doc?.selection.some(f=>f.name==='search'&&String(f.args.query).includes('is:merged')&&f.selection.some(s=>s.name==='nodes'));if(historyRead)entry.operation='history-search';
   if(fault.mode==='offline'){entry.terminal='connection-closed';req.socket.destroy();return;}
   if(fault.mode==='secondary'){res.writeHead(429,{'content-type':'application/json','retry-after':'2'});entry.status=429;res.end(JSON.stringify({message:'secondary rate limit'}));return;}
   if(fault.mode==='primary'){window.remaining=0;entry.remaining=0;res.writeHead(403,{'content-type':'application/json','x-ratelimit-remaining':'0','x-ratelimit-reset':String(entry.reset)});entry.status=403;res.end(JSON.stringify({message:'primary rate limit'}));return;}
   if(req.url!=='/graphql'){
    entry.operation=req.method==='GET'?'rest-read':'rest-write';entry.status=200;
    // The synthetic repository has measured-empty rulesets and no protection.
    const url=new URL(req.url,'http://127.0.0.1'),path=url.pathname;
    const repo=data.repos.find(r=>path.startsWith('/repos/'+r.nameWithOwner+'/'));
    if(!repo){entry.status=404;res.writeHead(404,{'content-type':'application/json'}).end(JSON.stringify({message:'Not Found'}));return;}
    let answer=[];
    if(path.endsWith('/activity')){const row=data.rows.find(r=>r.repository===repo&&'refs/heads/'+r.headRefName===url.searchParams.get('ref'));if(row)answer=[{activity_type:'push',after:row.headRefOid,actor:{login:row.author.login}}];}
    else if(!path.includes('/rules/branches/')&&!path.endsWith('/protection')){throw Error('unsupported REST shape');}
    if(path.endsWith('/protection')){entry.status=404;res.writeHead(404,{'content-type':'application/json'});res.end(JSON.stringify({message:'Not Found'}));return;}
    res.writeHead(200,{'content-type':'application/json','x-ratelimit-remaining':String(entry.remaining)});res.end(JSON.stringify(answer));return;
   }
   if(entry.operation==='unknown')throw Error('unknown operation');
   let pageRefusal=false;
   const resolve=(f,o)=>{
    if(o===root){
     if(f.name==='rateLimit')return {cost:1,remaining:entry.remaining,resetAt:new Date(entry.reset*1000).toISOString()};
     if(f.name==='viewer')return {login:'synthetic-viewer',organizations:connection([{login:'synthetic-lab',name:'Synthetic laboratory',avatarUrl:'',membersWithRole:connection(data.members),repositories:connection(data.repos)}]),repositories:connection(data.repos)};
     if(f.name==='organization')return f.args.login==='synthetic-lab'?{login:'synthetic-lab',name:'Synthetic laboratory',membersWithRole:connection(data.members),repositories:connection(data.repos)}:null;
     if(f.name==='repository')return data.repos.find(r=>r.name===f.args.name&&r.owner.login===f.args.owner)??null;
     if(f.name==='node')return data.rows.find(r=>r.id===f.args.id)??null;
     if(f.name==='nodes')return (f.args.ids??[]).map(id=>data.rows.find(r=>r.id===id)??null);
     if(f.name==='search'){
      const q=f.args.query??'';const rows=searchRows(data.rows,q);
      const start=Number(String(f.args.after??'cursor-0').split('-').at(-1)),end=Math.min(rows.length,start+(f.args.first??25));
      if((fault.mode==='partial'||(fault.historyPartial&&q.includes('is:merged')))&&start>0)pageRefusal=true;
      return {...connection(rows.slice(start,end)),issueCount:rows.length,pageInfo:{hasNextPage:end<rows.length,endCursor:`cursor-${end}`}};
     }
     if(f.name==='addPullRequestReview'){
      const input=f.args.input,pr=data.rows.find(r=>r.id===input.pullRequestId);if(!pr)throw Error('unknown mutation identity');
      const review={id:`REVIEW_${pr.number}`,state:({APPROVE:"APPROVED",REQUEST_CHANGES:"CHANGES_REQUESTED",COMMENT:"COMMENTED"})[input.event],submittedAt:date,author:{login:'synthetic-viewer'},commit:{oid:input.commitOID??pr.headRefOid},pullRequest:pr};
      pr.latestReviews=connection([review]);pr.reviews=connection([review]);return {pullRequestReview:review};
     }
     throw Error('unsupported root');
    }
    if(f.name==='pullRequest'&&f.args.number!==undefined)return data.rows.find(r=>r.number===f.args.number&&r.repository.nameWithOwner===o.nameWithOwner)??null;
    if(f.name==='pullRequests')return connection([]);
    if(f.name==='ref')return {target:{history:connection([])},associatedPullRequests:connection([])};
    if(f.name==='reviewThreads'&&o.number===51){const threads=structuredClone(o.reviewThreads);threads.nodes[0].isResolved=fault.thread;threads.nodes[0].isOutdated=fault.outdated;return threads;}
    return o[f.name]??null;
   };
   const root={};const response=project(doc.selection,root,resolve);
   if(pageRefusal){entry.status=503;res.writeHead(503).end();return;}
   if(fault.hold||(fault.holdHistory&&historyRead)){entry.held=true;await new Promise(resolve=>held.push(resolve));entry.released=true;}
   if(responseDelay)await new Promise(r=>setTimeout(r,responseDelay));
   entry.status=200;res.writeHead(200,{'content-type':'application/json','x-ratelimit-remaining':String(entry.remaining)});res.end(JSON.stringify({data:response}));
  }catch(error){entry.status=500;entry.error='unsupported-fixture';res.writeHead(500,{'content-type':'application/json'});res.end(JSON.stringify({error:'unsupported synthetic operation'}));console.error('provider fixture:',error.message);}
  finally {active--;entry.elapsed=performance.now()-started;entry.terminal??='response-sent';}
 });
 await new Promise(r=>server.listen(0,'127.0.0.1',r));
 return {url:`http://127.0.0.1:${server.address().port}`,ledger,fault,data,get lost(){return lost;},get heldCount(){return held.length;},release(){fault.hold=false;fault.holdHistory=false;for(const release of held.splice(0))release();},close:()=>new Promise(r=>server.close(r))};
}

export function searchRows(rows,q){
 const terms=q.match(/(?:[^\s"]+|"[^"]*")+/g)??[];
 return rows.filter(row=>terms.every(term=>{
  let negate=term.startsWith('-');if(negate)term=term.slice(1);
  const colon=term.indexOf(':');if(colon<0)throw Error('unsupported search term');const key=term.slice(0,colon),value=term.slice(colon+1).replace(/^"|"$/g,'');const who=value==='@me'?'synthetic-viewer':value;
  let match;
  if(key==='is'||key==='state')match=value==='pr'||(value==='open'&&row.state==='OPEN')||(value==='merged'&&row.state==='MERGED')||(value==='closed'&&['CLOSED','MERGED'].includes(row.state));
  else if(key==='author')match=row.author.login===who;
  else if(key==='review-requested')match=row.reviewRequests.nodes.some(r=>r.requestedReviewer.login===who);
  else if(key==='repo')match=row.repository.nameWithOwner===value;
  else if(key==='org'||key==='user')match=row.repository.owner.login===value;
  else if(key==='reviewed-by')match=row.reviews.nodes.some(r=>r.author.login===who);
  else if(key==='involves')match=row.author.login===who||row.reviewRequests.nodes.some(r=>r.requestedReviewer.login===who)||row.reviews.nodes.some(r=>r.author.login===who);
  else if(['created','updated','merged','closed'].includes(key)) {const at=row[`${key}At`];if(!at)match=false;else if(value.includes('..')){const [a,b]=value.split('..');match=at.slice(0,10)>=a&&at.slice(0,10)<=b;}else if(value.startsWith('>='))match=at.slice(0,10)>=value.slice(2);else match=at.slice(0,10)===value;}
  else if(key==='sort')match=true;
  else throw Error('unsupported search filter');
  return negate?!match:match;
 }));
}
