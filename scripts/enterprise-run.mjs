import {runChurn} from './enterprise/churn.mjs';
import {loadChurnRestart,captureChurnCache,saveCheckpoint} from './enterprise/churn-restart.mjs';
import {resourceCoverage,bindBrowserClock,summarizeResources,validateResourceResult,startupMeasurements,observeBrowserResources} from './enterprise/resources.mjs';
import {weightedQuotaProfile,quotaIdentity} from './enterprise/quota.mjs';
import {runQuotaScenario} from './enterprise/quota-scenario.mjs';
import {sourceFingerprint} from './enterprise/fingerprint.mjs';
import {baselineRows,loadConvergenceRestart,captureRestartCache,runConvergenceRestart} from './enterprise/convergence-restart.mjs';
import {runConvergence} from './enterprise/convergence.mjs';
import {loadWarmReplay,runWarmTail} from './enterprise/warm-tail.mjs';
import {runEnterpriseSoak,runEnterpriseTail,mountedReadyIdentities} from './enterprise/final-acceptance.mjs';
import {runReadyCompletion} from './enterprise/ready-completion.mjs';
import {assertNoStatsRequests,statsRequestCounts,validateEnterpriseMode} from './enterprise/stats-retirement.mjs';
import {runReadyProgress,runDualReadyProgress} from './enterprise/ready-progress.mjs';
import {defaultWorkload,normalizeWorkload,normalizeEnvironment,assertCompatibleWorkload,assertCompatibleEnvironment} from './enterprise/workload.mjs';
import assert from 'node:assert/strict';import {createServer} from 'node:http';import {mkdir,readFile,writeFile,copyFile,cp,readdir,stat} from 'node:fs/promises';import {createWriteStream,readFileSync} from 'node:fs';import {resolve,extname} from 'node:path';import {spawn,execFileSync,execFile} from 'node:child_process';import os from 'node:os';import {reconcile} from './enterprise/accounting.mjs';import {compareBaseline,validateBaseline} from './enterprise/budgets.mjs';import {randomBytes,createHash} from 'node:crypto';import {Readable} from 'node:stream';import {chromium,webkit} from 'playwright';import {startProvider} from './enterprise/provider.mjs';
const out=resolve(process.argv[2]);const engineName=process.argv[3]??'chromium';await mkdir(out,{recursive:true});assert.equal((await readdir(out)).length,0,'refuse to overwrite retained run artifacts');const mode=process.argv[4]??'gate';validateEnterpriseMode(mode);const quotaProfile=mode==='quota'?weightedQuotaProfile:undefined;const workload=normalizeWorkload({...defaultWorkload,...(quotaProfile?{quotaProfile:quotaIdentity(quotaProfile)}:{}),scenario:mode.startsWith('churn')?'churn':mode,...(mode.startsWith('churn')?{seed:Number(process.env.ENTERPRISE_SEED??9001),scheduleVersion:'churn-v2'}:{}),populations:{...defaultWorkload.populations,...(mode.startsWith('convergence')?{reviewing:236}:mode.startsWith('churn')?{reviewing:Number(process.env.ENTERPRISE_REVIEWING??150)}:{})}});const profile=resolve(out,'profile');const warm=!!process.argv[5];if(warm)await cp(resolve(process.argv[5]),profile,{recursive:true,errorOnExist:true,force:false});await mkdir(profile,{recursive:true});const ready=resolve(out,'ready.json'),secret=randomBytes(32).toString('hex');
const fingerprint=await sourceFingerprint();
const binary=process.env.ENTERPRISE_DRIVER??resolve('src-tauri/target/debug/enterprise-driver');
const browserAssets=[];async function hashAssets(directory){for(const entry of (await readdir(directory,{withFileTypes:true})).sort((a,b)=>a.name.localeCompare(b.name))){const path=resolve(directory,entry.name);if(entry.isDirectory())await hashAssets(path);else if(entry.isFile())browserAssets.push({path:path.slice(resolve('dist-harness-enterprise').length+1),sha256:createHash('sha256').update(await readFile(path)).digest('hex')});}}await hashAssets(resolve('dist-harness-enterprise'));
const harnessCode=await Promise.all(['scripts/enterprise-run.mjs','scripts/enterprise/provider.mjs','scripts/enterprise/churn-plan.mjs','scripts/enterprise/churn.mjs','scripts/enterprise/churn-restart.mjs','scripts/enterprise/workload.mjs','scripts/enterprise/stats-retirement.mjs','src/harness/enterprise.tsx','src/harness/enterpriseTransport.ts','src/harness/enterpriseFacts.ts'].map(async path=>({path,sha256:createHash('sha256').update(await readFile(path)).digest('hex')})));
const provider=await startProvider({convergence:mode.startsWith('convergence'),workload,quotaProfile});let warmReplay;try{warmReplay=mode==='tail-warm'?await loadWarmReplay(process.argv[5],profile,provider):undefined;}catch(error){await provider.close();throw error;}if(mode==='offline')provider.fault.mode='offline';
let churnRestart,churnCheckpoint,churnRestartError;
if(mode==='churn-restart'){try{churnRestart=await loadChurnRestart({inheritedProfile:process.argv[5],profile,provider,workload,expectedNativeBinaryHash:createHash('sha256').update(readFileSync(binary)).digest('hex')});}catch(error){churnRestartError=error;}}
let convergenceRestart;if(mode==='convergence-restart')convergenceRestart=await loadConvergenceRestart(process.argv[5],profile,provider);if(mode==='convergence')await writeFile(resolve(out,'convergence-baseline.json'),JSON.stringify(baselineRows(provider.data),null,2));
await writeFile(resolve(out,'metadata.json'),JSON.stringify({seed:workload.seed,workload,churnIntegrationOnly:process.env.ENTERPRISE_CHURN_INTEGRATION_ONLY==='1',engine:engineName,mode,warm,warmReplay,inheritedProfile:warm?resolve(process.argv[5]):null,browserAssets,harnessCode,build:'debug native + React profiling production browser',strictMode:false,commit:execFileSync('git',['rev-parse','HEAD'],{encoding:'utf8'}).trim(),dirty:execFileSync('git',['status','--porcelain'],{encoding:'utf8'}).trim().length>0,patchHash:fingerprint,sourceTree:execFileSync('git',['rev-parse','HEAD^{tree}'],{encoding:'utf8'}).trim(),nativeBinarySha256:createHash('sha256').update(readFileSync(binary)).digest('hex'),platform:process.platform,architecture:process.arch,osRelease:os.release(),cpu:os.cpus()[0].model,cores:os.cpus().length,memoryBytes:os.totalmem(),cadenceSeconds:60,excluded:['packaged Tauri IPC/WKWebView','installed mobile Rust proxy and lifecycle','real GitHub cost and enterprise SLA'],startedAt:new Date().toISOString()},null,2));const resourceDisabled=process.env.ENTERPRISE_RESOURCE_DISABLED==='1';const config=resolve(out,'private-config.json');await writeFile(config,JSON.stringify({profile,provider:provider.url,bridge_secret:secret,ready,resource_disabled:resourceDisabled}),{mode:0o600});await writeFile(resolve(out,'manifest.json'),JSON.stringify(provider.manifest,null,2));
const parentStarted=performance.now();const child=spawn(binary,[config],{stdio:['ignore','pipe','pipe']});child.stdout.pipe(createWriteStream(resolve(out,'native.stdout')));child.stderr.pipe(createWriteStream(resolve(out,'native.stderr')));let exit;const exited=new Promise(r=>child.once('exit',(code,signal)=>{exit={code,signal};r(exit);}));const delay=ms=>new Promise(r=>setTimeout(r,ms));let bridge,proxy,browser,closing=false,browserResources;const resourceStartup={};const resourceDetail={};let resourceBrowser={records:[],lost:false};const contexts=[],pages=[],bridgeCommands=[];const result={engine:engineName,scope:'native-connected production queues and detail',pass:false,mode,warm,warmReplay,samples:[],errors:[],expectedFailures:[],phases:[],quotaModel:{graphqlCapacity:5000,restCapacity:5000,costPerReceivedDocument:1,windowSeconds:120,meaning:'synthetic accounting only; not GitHub query cost'}};
result.workload=workload;if(quotaProfile)result.quotaModel={profile:quotaProfile,identity:quotaIdentity(quotaProfile),meaning:'synthetic shape weights; not GitHub pricing; documents and charged totals are distinct'};
// Independent parent sampling runs identically in both resource observer arms.
const parentSamples=[];let parentPending=Promise.resolve(),parentLost=false;
const parentSize=async name=>{try{return {status:'measured',bytes:(await stat(resolve(profile,name))).size};}catch(error){return {status:error.code==='ENOENT'?'absent':'unknown'};}};
const parentSample=()=>{parentPending=parentPending.then(async()=>{
 if(parentSamples.length>=100000){parentLost=true;return;}
 const rss=await new Promise(done=>execFile('ps',['-o','rss=','-p',String(child.pid)],{timeout:2000},(error,stdout)=>{const kib=Number(stdout.trim());done(!error&&Number.isFinite(kib)&&kib>0?{status:'measured',bytes:kib*1024}:{status:'unknown',reason:'process_unavailable'});}));
 parentSamples.push({at:performance.now()-parentStarted,nativeRss:rss,nodeHarnessRss:process.memoryUsage().rss,db:await parentSize('headstate.db'),walRetained:await parentSize('headstate.db-wal')});
 });return parentPending;};
await parentSample();const parentTimer=setInterval(()=>void parentSample(),1000);
async function artifactBytes(directory){let total=0;for(const entry of await readdir(directory,{withFileTypes:true})){const path=resolve(directory,entry.name);if(entry.isDirectory())total+=await artifactBytes(path);else if(entry.isFile())total+=(await stat(path)).size;}return total;}
const nativeCall=(role,command,args={})=>{bridgeCommands.push(command);return fetch(`${bridge}/call/${role}/${command}`,{method:'POST',headers:{'x-enterprise-secret':secret,'content-type':'application/json'},body:JSON.stringify(args)});};
const control=async action=>{const r=await fetch(`${bridge}/control/${action}`,{method:'POST',headers:{'x-enterprise-secret':secret}});assert.equal(r.status,200);return r.json();};
try {
 if(churnRestartError)throw churnRestartError;
 assert.ok(!(mode==='baseline'&&resourceDisabled),'startup baseline requires enabled raw resource evidence');
 for(let n=0;n<300;n++){if(exit)throw Error('native exited before ready');try{bridge=JSON.parse(await readFile(ready,'utf8')).bridge;break;}catch{}await delay(100);}assert.ok(bridge,'native ready');
 await parentSample();
 if(churnRestart)result.churnRestartProvenance=churnRestart.sourceProvenance;
 if(churnRestart)await captureChurnCache({nativeCall,provider,state:churnRestart,out});
 if(convergenceRestart)await captureRestartCache({nativeCall,provider,state:convergenceRestart,out});
 proxy=createServer(async(req,res)=>{try{
  const pathname=new URL(req.url,'http://127.0.0.1').pathname;
  if(pathname==='/workload.json'){res.writeHead(200,{'content-type':'application/json'});res.end(JSON.stringify(workload));return;}
  if(pathname.startsWith('/bridge/')){
   let body='';for await(const c of req)body+=c;
   if(pathname.includes('/call/'))bridgeCommands.push(pathname.split('/').at(-1));
   const up=await fetch(bridge+pathname.slice(7),{method:req.method,headers:{'x-enterprise-secret':secret,'content-type':'application/json'},...(req.method==='POST'?{body}:{})});
   res.writeHead(up.status,{'content-type':up.headers.get('content-type')??'application/json'});
   if(pathname.includes('/events/')){const stream=Readable.fromWeb(up.body);stream.on('error',()=>{if(!closing&&mode!=='crash')result.errors.push({kind:'unexpected-native-event-stream-close'});res.destroy();});stream.pipe(res);res.on('close',()=>stream.destroy());}else {res.end(await up.text());}return;
  }
  const file=resolve('dist-harness-enterprise','.'+(pathname==='/'?'/harness/enterprise.html':pathname));assert.ok(file.startsWith(resolve('dist-harness-enterprise')+'/'));
  res.writeHead(200,{'content-type':({'.html':'text/html','.js':'text/javascript','.css':'text/css','.woff2':'font/woff2'})[extname(file)]??'application/octet-stream'});res.end(await readFile(file));
 }catch{res.writeHead(500).end('Harness transport failure');}});await new Promise(r=>proxy.listen(0,'127.0.0.1',r));
 browser=await ({chromium,webkit})[engineName].launch({headless:true});result.version=browser.version();result.environment=normalizeEnvironment({platform:process.platform,architecture:process.arch,osRelease:os.release(),cpu:os.cpus()[0].model,build:'debug native + React profiling production browser',strictMode:false,browser:{engine:engineName,version:result.version},sourceRevision:execFileSync('git',['rev-parse','HEAD'],{encoding:'utf8'}).trim()});if(process.env.ENTERPRISE_BASELINE){result.admission={accepted:false,stage:'before-measured-work'};const baseline=validateBaseline(JSON.parse(await readFile(process.env.ENTERPRISE_BASELINE,'utf8')));assertCompatibleWorkload(result.workload,baseline.workload);assertCompatibleEnvironment(result.environment,baseline.environments?.[engineName]);result.admission.accepted=true;}
 for(const role of (mode==='ready-progress'?['desktop']:['desktop','paired'])){const context=await browser.newContext({viewport:role==='paired'?{width:430,height:900}:{width:1440,height:1000}});contexts.push(context);await context.tracing.start({screenshots:true,snapshots:true});const page=await context.newPage();page.setDefaultTimeout(30000);page.on('pageerror',e=>result.errors.push({role,error:e.message}));pages.push(page);}
 if(!resourceDisabled){browserResources=await observeBrowserResources(pages,engineName);await browserResources.startPhase('startup',control);}
 for(const [i,page] of pages.entries()){const role=i?'paired':'desktop';await page.goto(`http://127.0.0.1:${proxy.address().port}/?role=${role}${mode.startsWith('churn')?'&scenario=churn':mode==='quota'?'&scenario=quota':mode.startsWith('convergence')?'&scenario=convergence':mode==='ready-completion'?'&scenario=ready-completion':mode.startsWith('ready-')?'&scenario=ready-progress':''}`);}
 result.longTaskCapability=await pages[0].evaluate(()=>window.__enterprise.longTasks===null?{supported:false,value:null,reason:'PerformanceObserver longtask entry unsupported in this engine'}:{supported:true});
 if(mode.startsWith('churn')){churnCheckpoint=await runChurn({pages,provider,result,out,control,workload,state:churnRestart});}else if(mode==='quota'){result.samples.push({role:'desktop'},{role:'paired'});await runQuotaScenario({pages,provider,result,out,profile,control,nativeCall,quotaProfile});}else if(mode==='convergence-restart'){await runConvergenceRestart({pages,provider,result,out,profile,state:convergenceRestart,control});}else if(mode==='convergence'){await runConvergence({pages,provider,result,out,profile,control,nativeCall});}else if(mode==='tail-warm'){await runWarmTail({pages,provider,result,nativeCall,control});}else if(mode==='ready-completion'){result.samples.push({role:'desktop'},{role:'paired'});await runReadyCompletion({pages,provider,result,out,control,nativeCall});}else if(mode.startsWith('ready-dual-')){result.samples.push({role:'desktop'},{role:'paired'});await runDualReadyProgress({pages,provider,result,out,control,nativeCall,lifecycle:mode.includes('lifecycle'),leader:mode.endsWith('paired')?'paired':'desktop'});}else if(mode==='ready-progress'){result.samples.push({role:'desktop'});await runReadyProgress({page:pages[0],nativeCall,provider,result,out});}else{
 await control('start');
 for(const [i,page] of pages.entries()){
  await page.getByTestId('counts').filter({hasText:'Authored 50 / Reviewing 150'}).waitFor({timeout:120000});
  if(mode!=='offline')await page.getByRole('heading',{name:/Ready for review \(150\)/}).waitFor({timeout:30000});
  const inventory=await page.evaluate(()=>window.__enterprise.inventory());const open=provider.data.rows.filter(r=>r.state==='OPEN');assert.deepEqual(inventory.authored,open.filter(r=>r.author.login==='synthetic-viewer').map(r=>`${r.repository.nameWithOwner}/${r.number}`).sort());assert.deepEqual(inventory.reviewing,open.filter(r=>r.reviewRequests.nodes.some(q=>q.requestedReviewer.login==='synthetic-viewer')).map(r=>`${r.repository.nameWithOwner}/${r.number}`).sort());
  if(mode!=='offline')assert.deepEqual(await mountedReadyIdentities(page),inventory.reviewing,'exact mounted production Ready inventory');
  result.samples.push({role:i?'paired':'desktop',firstUsefulQueue:await page.evaluate(()=>window.__enterprise.firstUsefulQueue),fullQueue:await page.evaluate(()=>window.__enterprise.fullQueue)});
  await page.screenshot({path:resolve(out,`queue-${i}.png`),fullPage:false});
  const detailStart=await page.evaluate(()=>performance.now());resourceDetail[i]={start:detailStart};
  await page.getByRole('button',{name:/Synthetic review 51(?:\D|$)/}).first().click();
  if(mode==='offline'){await delay(1000);const approve=page.getByRole('button',{name:'Approve',exact:true});assert.ok(await approve.count()===0||await approve.first().isDisabled(),'offline retained inventory grants no fresh approval authority');continue;}
  await page.getByText('Synthetic description 51',{exact:false}).waitFor();
  resourceDetail[i].visible=await page.evaluate(()=>performance.now());result.samples[i].firstUsefulDetail=resourceDetail[i].visible-detailStart;
  assert.equal(await page.getByPlaceholder('Leave a comment (required to request changes)…').isEditable(),true);resourceDetail[i].draft=await page.evaluate(()=>performance.now());result.samples[i].draftReadyMs=resourceDetail[i].draft-detailStart;
  await page.waitForFunction(()=>[...document.querySelectorAll('button')].some(b=>b.textContent==='Approve'&&!b.disabled),null,{timeout:30000});resourceDetail[i].action=await page.evaluate(()=>performance.now());result.samples[i].freshActionReadyMs=resourceDetail[i].action-detailStart;
  await page.screenshot({path:resolve(out,`detail-${i}.png`),fullPage:false});
 }
 if(['authored','gate','fault','soak','load'].includes(mode)){
  const expected=provider.data.rows.filter(r=>r.state==='OPEN'&&r.author.login==='synthetic-viewer').map(r=>`${r.repository.nameWithOwner}/${r.number}`).sort();
  for(const [i,page] of pages.entries()){
   await page.getByRole('button',{name:'Back to list',exact:true}).click();
   await page.getByRole('button',{name:'My PRs',exact:true}).click();
   const rows=page.locator('div[role="button"]').filter({hasText:/Synthetic review [0-9]+/});
   await rows.first().waitFor();
   const identities=await rows.evaluateAll(nodes=>nodes.map(node=>{const text=node.textContent??'';return `${text.match(/synthetic-lab\/repo-[0-9]+/)?.[0]}/${text.match(/Synthetic review ([0-9]+)/)?.[1]}`;}).sort());
   assert.deepEqual(identities,expected,'actual mounted PrList identities');
   await page.screenshot({path:resolve(out,`authored-${i}.png`),fullPage:false});
   await page.getByRole('button',{name:/Synthetic review 1(?:\D|$)/}).first().click();
   await page.getByText('Synthetic description 1',{exact:false}).waitFor();
   await page.getByRole('button',{name:'Back to list',exact:true}).click();
   await page.getByRole('button',{name:'To Review',exact:true}).click();
   await page.getByRole('button',{name:/Synthetic review 51(?:\D|$)/}).first().click();
   await page.getByText('Synthetic description 51',{exact:false}).waitFor();
  }
  result.phases.push({name:'mounted-authored-PrList-and-detail-both-roles',applied:true,exercised:true,converged:true,rowsPerRole:50});
 }
 if(mode==='crash'){
  await control('hold-commit');await control('wake');let held=false;
  for(let n=0;n<180&&!held;n++){await delay(1000);held=(await control('status')).commitHeld;}
  assert.ok(held,'interruption must occur inside actual outer SQLite transaction');
  const before=JSON.parse(execFileSync('python3',['scripts/enterprise/inspect-profile.py',profile],{encoding:'utf8'}));
  await writeFile(resolve(out,'before-interruption.json'),JSON.stringify(before,null,2));
  child.kill('SIGKILL');await exited;
  const reopened=JSON.parse(execFileSync('python3',['scripts/enterprise/inspect-profile.py',profile],{encoding:'utf8'}));assert.equal(reopened.integrity,'ok');assert.deepEqual(reopened.queues,before.queues,'interrupted outer commit does not advance durable authority');
  result.phases.push({name:'actual-outer-transaction-interruption-reopen',applied:true,exercised:true,converged:true});
 }
 if(!['baseline','crash','offline','authored'].includes(mode)){
 if(mode!=='retirement'){
 // Real mounted multi-thread moves: invalidate through the actual QueryClient,
 // which reaches native get_pr_detail; never publish a canned receipt.
 for(const page of pages){
  await page.getByText('Open report',{exact:true}).click();
  const textarea=page.getByPlaceholder('Reply…').first();await textarea.fill('Retained synthetic backward draft');
  await textarea.evaluate(node=>{node.focus();node.setSelectionRange(3,17,'backward');window.__continuityNode=node;});
 }
 for(const [resolved,outdated] of [[true,false],[false,false],[false,true],[false,false]]){
  provider.fault.thread=resolved;provider.fault.outdated=outdated;
  await Promise.all(pages.map(page=>page.evaluate(()=>window.__enterprise.refreshDetail())));
  for(const page of pages){const order=await page.getByRole('button',{name:/synthetic.rs:[12]/}).allTextContents();assert.equal(order[0].includes('synthetic.rs:2'),resolved||outdated,'actual active/settled DOM order');const state=await page.evaluate(()=>{const n=window.__continuityNode;return {same:n?.isConnected,focus:document.activeElement===n,draft:n?.value,start:n?.selectionStart,end:n?.selectionEnd,direction:n?.selectionDirection,disclosure:[...document.querySelectorAll('details')].find(d=>d.querySelector('summary')?.textContent==='Open report')?.open};});assert.deepEqual(state,{same:true,focus:true,draft:'Retained synthetic backward draft',start:3,end:17,direction:'backward',disclosure:true});}
 }
 result.continuityMoves=4;
 const action=pages[0];provider.fault.delay=300;
 const actionStart=await action.evaluate(()=>performance.now());await action.getByRole('button',{name:'Approve',exact:true}).first().click();
 await action.getByRole('button',{name:'Working…',exact:true}).first().waitFor();
 result.actionPendingMs=await action.evaluate(start=>performance.now()-start,actionStart);
 await action.getByRole('button',{name:'Approved',exact:true}).first().waitFor({timeout:30000});
 result.actionConfirmedMs=await action.evaluate(start=>performance.now()-start,actionStart);provider.fault.delay=0;
 assert.equal(provider.ledger.filter(e=>e.operation==='review-write').length,1,'one write for one interaction');
 // Observe real scheduler publication -> sourceRefreshHooks -> acceptDetailFacts.
 const observedStart=performance.now();
 await pages[1].getByRole('button',{name:'Approved',exact:true}).first().waitFor({timeout:120000});
 result.pairedApprovalObservedMs=performance.now()-observedStart;result.approvalPropagation='automatic production source publication and detail revalidation';
 await pages[1].getByRole('button',{name:'Back to list',exact:true}).click();
 await pages[1].getByRole('button',{name:/Synthetic review 52(?:\D|$)/}).first().click();
 await pages[1].getByText('Synthetic description 52',{exact:false}).waitFor();
 provider.fault.delay=300;const pairedActionStart=performance.now();
 await pages[1].getByRole('button',{name:'Approve',exact:true}).first().click();
 await pages[1].getByRole('button',{name:'Working…',exact:true}).first().waitFor();
 result.pairedActionPendingMs=performance.now()-pairedActionStart;
 await pages[1].getByRole('button',{name:'Approved',exact:true}).first().waitFor();
 result.pairedActionConfirmedMs=performance.now()-pairedActionStart;provider.fault.delay=0;
 assert.equal(provider.ledger.filter(e=>e.operation==='review-write').length,2,'exactly two deliberate review writes');

 }
 if(mode==='fault'||mode==='retirement'){
  await pages[0].getByPlaceholder('Reply…').first().evaluate(node=>{node.focus();node.setSelectionRange(3,17,'backward');window.__continuityNode=node;});
  for(const fault of mode==='fault'?['partial','secondary','primary','offline']:[]){
   const phase={name:fault,applied:true,exercised:false,converged:false};result.phases.push(phase);
   const before=provider.ledger.length,started=performance.now();provider.fault.mode=fault;
   const positions=await Promise.all(pages.map(p=>p.evaluate(()=>window.__enterprise.telemetry.length)));
   await Promise.all(pages.map(p=>p.evaluate(()=>window.__enterprise.refreshQueues())));
   await control('wake');
   const encountered=()=>provider.ledger.slice(before).some(e=>e.fault===fault&&(fault==='partial'?e.status===503:fault==='secondary'?e.status===429:fault==='primary'?e.status===403:e.terminal==='connection-closed'));
   for(let n=0;n<180&&!encountered();n++){await delay(1000);for(const page of pages)assert.equal(await page.getByTestId('counts').textContent(),'Authored 50 / Reviewing 150','retained inventory while awaiting '+fault);}
   assert.ok(encountered(),'actual production transport encountered '+fault);phase.exercised=true;
   for(const page of pages)assert.equal(await page.getByTestId('counts').textContent(),'Authored 50 / Reviewing 150','retained inventory during '+fault);
   await pages[0].getByText('Synthetic description 51',{exact:false}).waitFor();
   const draft=await pages[0].getByPlaceholder('Reply…').first().inputValue();assert.equal(draft,'Retained synthetic backward draft');
   const retained=await pages[0].evaluate(()=>{const n=window.__continuityNode;return {same:n.isConnected,focused:document.activeElement===n,start:n.selectionStart,end:n.selectionEnd,direction:n.selectionDirection,disclosure:[...document.querySelectorAll('details')].some(d=>d.open&&d.querySelector('summary')?.textContent==='Open report')};});assert.deepEqual(retained,{same:true,focused:true,start:3,end:17,direction:'backward',disclosure:true});
   for(const [i,p] of pages.entries())result.expectedFailures.push(...await p.evaluate(start=>window.__enterprise.telemetry.slice(start).filter(e=>e.kind==='call'&&e.ok===false&&['get_reviewing','refresh_now','get_pr_detail'].includes(e.name)),positions[i]));
   Object.assign(phase,{elapsedMs:performance.now()-started,providerReceipts:provider.ledger.length-before,retained:true});
   const recoveredFrom=provider.ledger.length;
   provider.fault.mode='healthy';await delay(fault==='primary'?Math.max(2500,...provider.ledger.slice(before).filter(e=>e.status===403).map(e=>e.reset*1000-Date.now()+1000)):2500);await Promise.all(pages.map(p=>p.evaluate(()=>window.__enterprise.refreshQueues())));
   await control('wake');
   const recovered=()=>provider.ledger.slice(recoveredFrom).some(e=>e.fault==='healthy'&&e.status===200);
   for(let n=0;n<180&&!recovered();n++)await delay(1000);
   assert.ok(recovered(),'actual healthy provider receipt after '+fault);
   phase.converged=await pages[0].getByTestId('counts').textContent()==='Authored 50 / Reviewing 150';assert.ok(phase.converged);
   phase.recoveryMeaning='actual successful provider receipt and retained accepted inventory; final durable traversal is separately inspected';
  }
  if(mode==='fault'){
  const sqlBefore=(await readFile(resolve(profile,'native.ndjson'),'utf8')).length;
  const durableBefore=execFileSync('python3',['scripts/enterprise/inspect-profile.py',profile],{encoding:'utf8'});
  await control('fail-queue-write');await pages[0].evaluate(()=>window.__enterprise.refreshQueues());await control('wake');
  let failedTransaction=false;
  for(let n=0;n<180&&!failedTransaction;n++){await delay(1000);const entries=(await readFile(resolve(profile,'native.ndjson'),'utf8')).slice(sqlBefore).trim().split('\n').filter(Boolean).map(JSON.parse);failedTransaction=entries.some(e=>e.operation==='queue-transaction'&&e.stage==='cancelled');}
  assert.ok(failedTransaction,'actual failed outer SQLite transaction');
  const durableAfter=execFileSync('python3',['scripts/enterprise/inspect-profile.py',profile],{encoding:'utf8'});
  assert.deepEqual(JSON.parse(durableAfter).queues,JSON.parse(durableBefore).queues,'aborted transaction retained accepted revisions');
  assert.equal(await pages[0].getByTestId('counts').textContent(),'Authored 50 / Reviewing 150');
  await control('recover-queue-write');await pages[0].evaluate(()=>window.__enterprise.refreshQueues());
  result.phases.push({name:'sqlite-aborted-outer-write',applied:true,exercised:true,converged:true,retained:true});
  }
  await control('revoke');
  for(let n=0;n<100;n++){if((await control('status')).pairedClosed)break;await delay(100);}
  assert.equal((await control('status')).pairedClosed,true);
  const refused=await fetch(`${bridge}/call/paired/get_viewer`,{method:'POST',headers:{'x-enterprise-secret':secret,'content-type':'application/json'},body:'{}'});assert.ok(!refused.ok,'retired certificate refused');
  await control('repair');
  const repaired=await fetch(`${bridge}/call/paired/get_viewer`,{method:'POST',headers:{'x-enterprise-secret':secret,'content-type':'application/json'},body:'{}'});assert.equal(repaired.status,200);
  result.phases.push({name:'actual-mtls-revoke-same-certificate-repair',applied:true,exercised:true,converged:true});
  const outOfOrderFrom=provider.ledger.length;provider.fault.delays=[500,0];
  const replies=await Promise.all([nativeCall('desktop','get_pr_detail',{repo:'synthetic-lab/repo-3',number:53}),nativeCall('paired','get_pr_detail',{repo:'synthetic-lab/repo-4',number:54})]);for(const r of replies)assert.equal(r.status,200);
  const receipts=provider.ledger.slice(outOfOrderFrom).filter(e=>e.bucket==='graphql'&&e.status===200);assert.ok(receipts.length>=2);assert.ok(receipts.some((a,i)=>receipts.slice(i+1).some(b=>a.at+a.elapsed>b.at+b.elapsed)),'provider replies completed out of request order');
  const beforeSnapshot=provider.ledger.length;const status=await control('status');assert.equal(provider.ledger.length,beforeSnapshot,'active admission snapshot adds no provider request');assert.ok(status.admission.graphql.remaining<=Math.min(...receipts.map(e=>e.remaining)),'late higher remaining cannot revive spent quota');
  result.phases.push({name:'real-out-of-order-replies-and-passive-admission-snapshot',applied:true,exercised:true,converged:true});
 }

 }

 if(mode==='soak'||mode==='load'){
  await pages[0].getByRole('button',{name:'Back to list',exact:true}).click();
  await pages[0].getByRole('button',{name:/Synthetic review 53(?:\D|$)/}).first().click();
  await pages[0].getByText('Synthetic description 53',{exact:false}).waitFor();
  provider.fault.holdSearch={list:'reviewing',remaining:1};
  for(let n=0;n<150&&provider.heldCount===0;n++)await delay(1000);
  assert.ok(provider.heldCount>0,'actual reviewing background transport must be outstanding');
  assert.ok(provider.ledger.some(e=>e.held&&!e.released&&e.searches?.some(search=>search.query.includes('review-requested:@me'))));
  provider.fault.delay=300;const started=performance.now();await pages[0].getByRole('button',{name:'Approve',exact:true}).first().click();
  await pages[0].getByRole('button',{name:'Working…',exact:true}).first().waitFor();
  const pendingMs=performance.now()-started;
  await pages[0].getByRole('button',{name:'Approved',exact:true}).first().waitFor();
  result.phases.push({name:'foreground-approval-during-held-production-reviewing-search',applied:true,exercised:true,converged:true,pendingMs,confirmedMs:performance.now()-started});provider.fault.delay=0;provider.release();
 }
 if(mode==='tail')await runEnterpriseTail({pages,provider,result,nativeCall});
 if(mode==='soak')await runEnterpriseSoak({pages,provider,result,out,profile,nativeCall});
 }
 for(const [i,page] of pages.entries()) {
  const failed=await page.evaluate(()=>window.__enterprise.telemetry.filter(e=>e.kind==='call'&&e.ok===false&&e.name!=='diag_log'));
  if(mode==='offline')result.expectedFailures.push(...failed.filter(e=>['get_viewer','get_pr_detail','get_reviewing','refresh_now','get_review_gates'].includes(e.name)));
  result.errors.push(...failed.filter(e=>!result.expectedFailures.some(expected=>expected.name===e.name&&expected.at===e.at)).map(e=>({role:i?'paired':'desktop',command:e.name})));
 }
 for(const [i,page] of pages.entries()){const raw=await page.evaluate(()=>({measurement:window.__enterprise.measurement,commits:window.__enterprise.commits,longTasks:window.__enterprise.longTasks,firstUsefulQueue:window.__enterprise.firstUsefulQueue,fullQueue:window.__enterprise.fullQueue,telemetryEntries:window.__enterprise.telemetry.length}));assert.equal(raw.measurement.lost,false,'bounded browser telemetry intact');resourceStartup[i?'paired':'desktop']={...raw,detail:resourceDetail[i]};const metrics=mode==='baseline'?startupMeasurements(resourceStartup[i?'paired':'desktop']):{uiCommits:raw.commits.length,uiCommitMs:raw.commits.reduce((n,c)=>n+c.actual,0),longTaskCount:raw.longTasks?.length??null,longTaskMs:raw.longTasks?.reduce((n,t)=>n+t.duration,0)??null};assert.ok(metrics.uiCommits>0,'profiling renderer must emit actual commits');Object.assign(result.samples[i],metrics);}
 await parentSample();if(browserResources)await browserResources.endPhase('startup',control);
 result.pass=result.errors.length===0;
}catch(error){result.failure=error.message;process.exitCode=1;}
finally {
 closing=true;clearInterval(parentTimer);await parentPending;
 if(browserResources){try{resourceBrowser=await browserResources.stop();}catch(error){result.pass=false;result.failure??=error.message;}}
 for(const [i,page] of pages.entries()){try{await writeFile(resolve(out,`ui-${i}.json`),JSON.stringify(await page.evaluate(()=>window.__enterprise)));await page.screenshot({path:resolve(out,`final-${i}.png`)});}catch{}}
 for(const [i,c] of contexts.entries()){await c.tracing.stop({path:resolve(out,`trace-${i}.zip`)});await c.close();}
 if(browser)await browser.close();provider.release();
 if(bridge&&!exit){await control('stop');await Promise.race([exited,delay(10000)]);}
 if(!exit){child.kill('SIGTERM');await Promise.race([exited,delay(2000)]);if(!exit){child.kill('SIGKILL');await exited;}}
 if(proxy){proxy.closeAllConnections();await new Promise(r=>proxy.close(r));}await provider.close();
 try{await copyFile(resolve(profile,'native.ndjson'),resolve(out,'native.ndjson'));if(mode==='crash'){result.shutdown={intentionalInterruption:true};assert.equal(exit.signal,'SIGKILL');}else{result.shutdown=JSON.parse(await readFile(resolve(profile,'shutdown.json'),'utf8'));assert.equal(result.shutdown.telemetryLost,false);assert.equal(exit.code,0);}}catch(e){result.pass=false;result.failure??=e.message;}
 try{const events=(await readFile(resolve(out,'native.ndjson'),'utf8')).trim().split('\n').map(JSON.parse);assert.equal(provider.lost,false,'bounded provider ledger intact');result.statsRetirement=statsRequestCounts(provider.ledger,bridgeCommands);assertNoStatsRequests(provider.ledger,bridgeCommands);result.accounting=reconcile(events,provider.ledger,{interrupted:mode==='crash'});if(!resourceDisabled&&mode==='baseline'){const evidence={schema:1,contract:'startup',samplerIntervalMs:1000,coverage:resourceCoverage,lost:resourceBrowser.lost||result.shutdown.telemetryLost,native:events.filter(e=>e.stage==='resource'),browser:resourceBrowser.records,browserClock:bindBrowserClock(resourceBrowser.clock,events)};result.resources={artifact:'resources.json',evidence,summary:summarizeResources(evidence,engineName),startup:resourceStartup};validateResourceResult(result);await writeFile(resolve(out,'resources.json'),JSON.stringify(result.resources,null,2));}else result.resources={enabled:!resourceDisabled,reason:resourceDisabled?'runtime_disabled':'nonbaseline_phase_contract_not_asserted',rawNativeArtifact:'native.ndjson',browser:resourceBrowser};if(mode==='crash')assert.ok(result.accounting.unfinished.some(e=>e.operation==='queue-transaction'));await writeFile(resolve(out,'profile-summary.json'),execFileSync('python3',['scripts/enterprise/inspect-profile.py',profile]));}catch(e){result.pass=false;result.failure??=e.message;}
 result.exit=exit;result.providerReceipts=provider.ledger.length;
 const measuredRss=parentSamples.filter(s=>s.nativeRss.status==='measured').map(s=>s.nativeRss.bytes);
 result.parentObservation={wallMs:performance.now()-parentStarted,nativeRssPeak:measuredRss.length?Math.max(...measuredRss):null,nodeHarnessRssPeak:Math.max(...parentSamples.map(s=>s.nodeHarnessRss)),artifactBytes:await artifactBytes(out),commandLatencyMaxMs:null,lost:parentLost,semantics:'independent parent ps RSS, sampled retained DB/WAL; artifact bytes before final result/provider summaries'};
 try{const events=(await readFile(resolve(out,'native.ndjson'),'utf8')).trim().split('\n').map(JSON.parse);const begin=new Map(events.filter(e=>e.operation==='command'&&e.stage==='begin').map(e=>[e.id,e.ns]));const times=events.filter(e=>e.operation==='command'&&['complete','failed','cancelled'].includes(e.stage)&&begin.has(e.id)).map(e=>(e.ns-begin.get(e.id))/1e6);if(times.length)result.parentObservation.commandLatencyMaxMs=Math.max(...times);}catch{}
 result.nativeBinarySha256=createHash('sha256').update(readFileSync(binary)).digest('hex');
 await writeFile(resolve(out,'parent-resources.json'),JSON.stringify({samples:parentSamples,summary:result.parentObservation},null,2));
 const artifactSize=async name=>{try{return {status:'measured',bytes:(await stat(resolve(out,name))).size};}catch{return {status:'unknown',reason:'artifact_unavailable'};}};
 result.observerArtifacts={nativeRecorder:{capacityEvents:65536,memory:{status:'unknown',reason:'allocator_not_separable'},artifact:await artifactSize('native.ndjson')},resourceEvidence:await artifactSize('resources.json'),renderers:await Promise.all(['desktop','paired'].map(async(role,i)=>({role,telemetryEntries:resourceStartup[role]?.telemetryEntries??null,commitEntries:resourceStartup[role]?.commits?.length??null,longTaskEntries:resourceStartup[role]?.longTasks?.length??null,telemetryArtifact:await artifactSize(`ui-${i}.json`),traceArtifact:await artifactSize(`trace-${i}.zip`)})))};
 if(mode==='baseline'&&process.env.ENTERPRISE_BASELINE){try{result.baselineComparison=compareBaseline(result,JSON.parse(await readFile(process.env.ENTERPRISE_BASELINE,'utf8')));}catch(e){result.pass=false;result.failure??=e.message;}}
 if(churnCheckpoint)await saveCheckpoint({out,profile,provider,...churnCheckpoint,result});
 await writeFile(resolve(out,'provider.json'),JSON.stringify(provider.ledger,null,2));await writeFile(resolve(out,'result.json'),JSON.stringify(result,null,2));console.log(JSON.stringify(result));if(!result.pass)process.exitCode=1;
}
