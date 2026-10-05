import {runEnterpriseSoak,runEnterpriseTail,mountedReadyIdentities} from './enterprise/final-acceptance.mjs';
import {runReadyCompletion} from './enterprise/ready-completion.mjs';
import {runStatsConvergence} from './enterprise/stats-convergence.mjs';
import {runReadyProgress,runDualReadyProgress} from './enterprise/ready-progress.mjs';
import assert from 'node:assert/strict';import {createServer} from 'node:http';import {mkdir,readFile,writeFile,copyFile,cp,readdir} from 'node:fs/promises';import {createWriteStream,readFileSync} from 'node:fs';import {resolve,extname} from 'node:path';import {spawn,execFileSync} from 'node:child_process';import os from 'node:os';import {reconcile} from './enterprise/accounting.mjs';import {compareBaseline} from './enterprise/budgets.mjs';import {randomBytes,createHash} from 'node:crypto';import {Readable} from 'node:stream';import {chromium,webkit} from 'playwright';import {startProvider,manifest} from './enterprise/provider.mjs';
const out=resolve(process.argv[2]);const engineName=process.argv[3]??'chromium';await mkdir(out,{recursive:true});assert.equal((await readdir(out)).length,0,'refuse to overwrite retained run artifacts');const mode=process.argv[4]??'gate';const profile=resolve(out,'profile');const warm=!!process.argv[5];if(warm)await cp(resolve(process.argv[5]),profile,{recursive:true,errorOnExist:true,force:false});await mkdir(profile,{recursive:true});const ready=resolve(out,'ready.json'),secret=randomBytes(32).toString('hex');
const fingerprint=createHash('sha256').update(execFileSync('git',['diff','HEAD','--no-ext-diff']));for(const file of execFileSync('git',['ls-files','--others','--exclude-standard'],{encoding:'utf8'}).trim().split('\n').filter(Boolean))fingerprint.update(file).update(readFileSync(file));
const binary=process.env.ENTERPRISE_DRIVER??resolve('src-tauri/target/debug/enterprise-driver');
const browserAssets=[];async function hashAssets(directory){for(const entry of (await readdir(directory,{withFileTypes:true})).sort((a,b)=>a.name.localeCompare(b.name))){const path=resolve(directory,entry.name);if(entry.isDirectory())await hashAssets(path);else if(entry.isFile())browserAssets.push({path:path.slice(resolve('dist-harness-enterprise').length+1),sha256:createHash('sha256').update(await readFile(path)).digest('hex')});}}await hashAssets(resolve('dist-harness-enterprise'));
const provider=await startProvider();if(mode==='offline')provider.fault.mode='offline';
await writeFile(resolve(out,'metadata.json'),JSON.stringify({seed:9001,engine:engineName,mode,warm,inheritedProfile:warm?resolve(process.argv[5]):null,browserAssets,build:'debug native + React profiling production browser',strictMode:false,commit:execFileSync('git',['rev-parse','HEAD'],{encoding:'utf8'}).trim(),dirty:execFileSync('git',['status','--porcelain'],{encoding:'utf8'}).trim().length>0,patchHash:fingerprint.digest('hex'),sourceTree:execFileSync('git',['rev-parse','HEAD^{tree}'],{encoding:'utf8'}).trim(),nativeBinarySha256:createHash('sha256').update(readFileSync(binary)).digest('hex'),platform:process.platform,architecture:process.arch,osRelease:os.release(),cpu:os.cpus()[0].model,cores:os.cpus().length,memoryBytes:os.totalmem(),cadenceSeconds:60,excluded:['packaged Tauri IPC/WKWebView','installed mobile Rust proxy and lifecycle','real GitHub cost and enterprise SLA'],startedAt:new Date().toISOString()},null,2));const config=resolve(out,'private-config.json');await writeFile(config,JSON.stringify({profile,provider:provider.url,bridge_secret:secret,ready}),{mode:0o600});await writeFile(resolve(out,'manifest.json'),JSON.stringify(manifest,null,2));
const child=spawn(binary,[config],{stdio:['ignore','pipe','pipe']});child.stdout.pipe(createWriteStream(resolve(out,'native.stdout')));child.stderr.pipe(createWriteStream(resolve(out,'native.stderr')));let exit;const exited=new Promise(r=>child.once('exit',(code,signal)=>{exit={code,signal};r(exit);}));const delay=ms=>new Promise(r=>setTimeout(r,ms));let bridge,proxy,browser,closing=false;const contexts=[],pages=[];const result={engine:engineName,scope:'native-connected production queues and detail',pass:false,mode,warm,samples:[],errors:[],expectedFailures:[],phases:[],quotaModel:{graphqlCapacity:5000,restCapacity:5000,costPerReceivedDocument:1,windowSeconds:120,meaning:'synthetic accounting only; not GitHub query cost'}};
const nativeCall=(role,command,args={})=>fetch(`${bridge}/call/${role}/${command}`,{method:'POST',headers:{'x-enterprise-secret':secret,'content-type':'application/json'},body:JSON.stringify(args)});
const control=async action=>{const r=await fetch(`${bridge}/control/${action}`,{method:'POST',headers:{'x-enterprise-secret':secret}});assert.equal(r.status,200);return r.json();};
try {
 for(let n=0;n<300;n++){if(exit)throw Error('native exited before ready');try{bridge=JSON.parse(await readFile(ready,'utf8')).bridge;break;}catch{}await delay(100);}assert.ok(bridge,'native ready');
 proxy=createServer(async(req,res)=>{try{
  const pathname=new URL(req.url,'http://127.0.0.1').pathname;
  if(pathname.startsWith('/bridge/')){
   let body='';for await(const c of req)body+=c;
   const cachedRead=pathname.endsWith('/stats_board_cached')?{role:pathname.includes('/paired/')?'paired':'desktop',started:Date.now(),providerBefore:provider.ledger.length}:null;
   const up=await fetch(bridge+pathname.slice(7),{method:req.method,headers:{'x-enterprise-secret':secret,'content-type':'application/json'},...(req.method==='POST'?{body}:{})});
   res.writeHead(up.status,{'content-type':up.headers.get('content-type')??'application/json'});
   if(pathname.includes('/events/')){const stream=Readable.fromWeb(up.body);stream.on('error',()=>{if(!closing&&mode!=='crash')result.errors.push({kind:'unexpected-native-event-stream-close'});res.destroy();});stream.pipe(res);res.on('close',()=>stream.destroy());}else {const text=await up.text();res.end(text);if(cachedRead){result.cachedReads??=[];result.cachedReads.push({...cachedRead,finished:Date.now(),providerAfter:provider.ledger.length,status:up.status,callId:JSON.parse(text).callId});}}return;
  }
  const file=resolve('dist-harness-enterprise','.'+(pathname==='/'?'/harness/enterprise.html':pathname));assert.ok(file.startsWith(resolve('dist-harness-enterprise')+'/'));
  res.writeHead(200,{'content-type':({'.html':'text/html','.js':'text/javascript','.css':'text/css','.woff2':'font/woff2'})[extname(file)]??'application/octet-stream'});res.end(await readFile(file));
 }catch{res.writeHead(500).end('Harness transport failure');}});await new Promise(r=>proxy.listen(0,'127.0.0.1',r));
 browser=await ({chromium,webkit})[engineName].launch({headless:true});result.version=browser.version();if(process.env.ENTERPRISE_BASELINE){const baseline=JSON.parse(await readFile(process.env.ENTERPRISE_BASELINE,'utf8'));assert.equal(baseline.engines[engineName],result.version,'baseline engine version must match');assert.equal(baseline.environment.cpu,os.cpus()[0].model,'baseline CPU must match');assert.equal(baseline.environment.architecture,process.arch,'baseline architecture must match');assert.equal(baseline.environment.osRelease,os.release(),'baseline OS must match');assert.equal(baseline.environment.build,'debug native + React profiling production browser','baseline build must match');}
 for(const role of (mode==='ready-progress'?['desktop']:['desktop','paired'])){const context=await browser.newContext({viewport:role==='paired'?{width:430,height:900}:{width:1440,height:1000}});contexts.push(context);await context.tracing.start({screenshots:true,snapshots:true});const page=await context.newPage();page.setDefaultTimeout(30000);page.on('pageerror',e=>result.errors.push({role,error:e.message}));pages.push(page);await page.goto(`http://127.0.0.1:${proxy.address().port}/?role=${role}${mode==='ready-completion'?'&scenario=ready-completion':mode.startsWith('ready-')?'&scenario=ready-progress':''}`);}
 result.longTaskCapability=await pages[0].evaluate(()=>window.__enterprise.longTasks===null?{supported:false,value:null,reason:'PerformanceObserver longtask entry unsupported in this engine'}:{supported:true});
 if(mode==='ready-completion'){result.samples.push({role:'desktop'},{role:'paired'});await runReadyCompletion({pages,provider,result,out,control,nativeCall});}else if(mode==='stats-convergence'){result.samples.push({role:'desktop'},{role:'paired'});await runStatsConvergence({pages,provider,result,out,control});}else if(mode.startsWith('ready-dual-')){result.samples.push({role:'desktop'},{role:'paired'});await runDualReadyProgress({pages,provider,result,out,control,nativeCall,lifecycle:mode.includes('lifecycle'),leader:mode.endsWith('paired')?'paired':'desktop'});}else if(mode==='ready-progress'){result.samples.push({role:'desktop'});await runReadyProgress({page:pages[0],nativeCall,provider,result,out});}else{
 await control('start');
 for(const [i,page] of pages.entries()){
  await page.getByTestId('counts').filter({hasText:'Authored 50 / Reviewing 150'}).waitFor({timeout:120000});
  if(mode!=='offline')await page.getByRole('heading',{name:/Ready for review \(150\)/}).waitFor({timeout:30000});
  const inventory=await page.evaluate(()=>window.__enterprise.inventory());const open=provider.data.rows.filter(r=>r.state==='OPEN');assert.deepEqual(inventory.authored,open.filter(r=>r.author.login==='synthetic-viewer').map(r=>`${r.repository.nameWithOwner}/${r.number}`).sort());assert.deepEqual(inventory.reviewing,open.filter(r=>r.reviewRequests.nodes.some(q=>q.requestedReviewer.login==='synthetic-viewer')).map(r=>`${r.repository.nameWithOwner}/${r.number}`).sort());
  if(mode!=='offline')assert.deepEqual(await mountedReadyIdentities(page),inventory.reviewing,'exact mounted production Ready inventory');
  result.samples.push({role:i?'paired':'desktop',firstUsefulQueue:await page.evaluate(()=>window.__enterprise.firstUsefulQueue),fullQueue:await page.evaluate(()=>window.__enterprise.fullQueue)});
  await page.screenshot({path:resolve(out,`queue-${i}.png`),fullPage:false});
  const detailStart=await page.evaluate(()=>performance.now());
  await page.getByRole('button',{name:/Synthetic review 51(?:\D|$)/}).first().click();
  if(mode==='offline'){await delay(1000);const approve=page.getByRole('button',{name:'Approve',exact:true});assert.ok(await approve.count()===0||await approve.first().isDisabled(),'offline retained inventory grants no fresh approval authority');continue;}
  await page.getByText('Synthetic description 51',{exact:false}).waitFor();
  result.samples[i].firstUsefulDetail=await page.evaluate(start=>performance.now()-start,detailStart);
  assert.equal(await page.getByPlaceholder('Leave a comment (required to request changes)…').isEditable(),true);result.samples[i].draftReadyMs=await page.evaluate(start=>performance.now()-start,detailStart);
  await page.waitForFunction(()=>[...document.querySelectorAll('button')].some(b=>b.textContent==='Approve'&&!b.disabled),null,{timeout:30000});result.samples[i].freshActionReadyMs=await page.evaluate(start=>performance.now()-start,detailStart);
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
 if(!['retirement','contention'].includes(mode)){
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
 if(mode==='soak'||mode==='load')provider.fault.historyPartial=true;
 await pages[1].getByRole('button',{name:'Statistics',exact:true}).click();
 await pages[1].getByText('synthetic-lab',{exact:false}).first().waitFor({timeout:90000});
 await pages[1].waitForFunction(()=>window.__enterprise.telemetry.some(e=>e.name==='stats_board'&&e.ok===true),null,{timeout:90000});
 result.statsMounted=true;
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
   for(const [i,p] of pages.entries())result.expectedFailures.push(...await p.evaluate(start=>window.__enterprise.telemetry.slice(start).filter(e=>e.kind==='call'&&e.ok===false&&['get_reviewing','refresh_now','get_pr_detail','stats_board'].includes(e.name)),positions[i]));
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
  if(mode==='retirement'){const primed=await nativeCall('desktop','stats_board',{scopeKind:'repo',scopeValue:'synthetic-lab/repo-48',measure:'merged',days:1});assert.equal(primed.status,200,'actual Stats command establishes captured owner before demand');}
  const acquire=await nativeCall('paired','stats_demand',{request:{op:'acquire',scopeKind:'repo',scopeValue:'synthetic-lab/repo-49',measure:'merged',days:7}});assert.equal(acquire.status,200);const oldLease=(await acquire.json()).wire;
  const retirementBrowserStart=await pages[1].evaluate(()=>performance.now());
  await control('revoke');
  for(let n=0;n<100;n++){if((await control('status')).pairedClosed)break;await delay(100);}
  assert.equal((await control('status')).pairedClosed,true);
  const refused=await fetch(`${bridge}/call/paired/get_viewer`,{method:'POST',headers:{'x-enterprise-secret':secret,'content-type':'application/json'},body:'{}'});assert.ok(!refused.ok,'retired certificate refused');
  await control('repair');
  const repaired=await fetch(`${bridge}/call/paired/get_viewer`,{method:'POST',headers:{'x-enterprise-secret':secret,'content-type':'application/json'},body:'{}'});assert.equal(repaired.status,200);
  const oldRenew=await nativeCall('paired','stats_demand',{request:{op:'renew',handle:oldLease.handle,sequence:1}});assert.ok(!oldRenew.ok,'new pairing cannot renew retired incarnation lease');
  const reacquire=await nativeCall('paired','stats_demand',{request:{op:'acquire',scopeKind:'repo',scopeValue:'synthetic-lab/repo-49',measure:'merged',days:7}});assert.equal(reacquire.status,200);assert.notEqual((await reacquire.json()).wire.handle,oldLease.handle);
  result.phases.push({name:'actual-mtls-revoke-same-certificate-repair',applied:true,exercised:true,converged:true});
  provider.fault.hold=true;
  const oldBoard=nativeCall('desktop','stats_board',{scopeKind:'repo',scopeValue:'synthetic-lab/repo-50',measure:'opened',days:90});
  for(let n=0;n<100&&provider.heldCount===0;n++)await delay(100);
  assert.ok(provider.heldCount>0,'old-owner board reached actual provider');
  await control('retire-account');provider.release();
  const retired=await oldBoard;const retiredBody=await retired.json();await writeFile(resolve(out,'retired-board.json'),JSON.stringify(retiredBody,null,2));
  if(retired.ok){const board=retiredBody.value;assert.equal(board.viewer,'synthetic-viewer');assert.equal(board.backfill.state,'failed','useful original-owner measurements must explicitly refuse new registration');assert.deepEqual(board.owner,oldLease.owner,'original captured owner remains explicit');assert.equal(board.accumulating,false,'old results are not saved');}
  const afterRetirement=JSON.parse(execFileSync('python3',['scripts/enterprise/inspect-profile.py',profile],{encoding:'utf8'}));
  assert.equal(afterRetirement.ownerSlot,'replacement');assert.ok(afterRetirement.ownerGeneration>oldLease.owner.generation);assert.equal(afterRetirement.counts.stats_cache,0);assert.equal(afterRetirement.counts.pr_backfill_page,0);assert.equal(afterRetirement.counts.pr_history,0);assert.equal(afterRetirement.counts.pr_slice,0);assert.equal(afterRetirement.counts.pr_backfill_scope,0);
  result.phases.push({name:'pending-durable-stats-owner-retirement',applied:true,exercised:true,converged:true});
  await control('restore-account');
  if(mode==='fault'){
   // The still-mounted production hook owns renewal/reacquisition, not this controller.
   await pages[1].waitForFunction(start=>window.__enterprise.telemetry.some(e=>e.name==='stats_demand'&&e.at>=start&&e.demandOp==='renew'&&e.refusal==='expired-lease'),retirementBrowserStart,{timeout:75000});
   const retiredRenew=await pages[1].evaluate(start=>window.__enterprise.telemetry.find(e=>e.name==='stats_demand'&&e.at>=start&&e.demandOp==='renew'&&e.refusal==='expired-lease'),retirementBrowserStart);
   assert.ok(retiredRenew);result.expectedFailures.push(retiredRenew);
   let reacquired=false;
   for(let n=0;n<75&&!reacquired;n++){await delay(1000);assert.equal(await pages[1].getByTestId('counts').textContent(),'Authored 50 / Reviewing 150');assert.ok(await pages[1].getByText('Pull request activity',{exact:true}).isVisible());reacquired=await pages[1].evaluate(start=>window.__enterprise.telemetry.some(e=>e.name==='stats_demand'&&e.at>start&&e.demandOp==='acquire'&&e.ok===true),retiredRenew.at);}
   assert.ok(reacquired,'actual production demand timer reacquires after retired renewal');
   const calls=await pages[1].evaluate(start=>window.__enterprise.telemetry.filter(e=>e.name==='stats_demand'&&e.at>=start),retirementBrowserStart);
   assert.ok(calls.length<=4,'retirement recovery remains on bounded production timer');
   assert.equal(await pages[1].getByTestId('counts').textContent(),'Authored 50 / Reviewing 150');
   assert.ok(await pages[1].getByText('Pull request activity',{exact:true}).isVisible(),'Stats content remains mounted through retired lease recovery');
   result.phases.push({name:'mounted-stats-demand-automatic-reacquisition',applied:true,exercised:true,converged:true,calls:calls.length,renewRefusalAt:retiredRenew.at,acquiredAt:calls.find(e=>e.demandOp==='acquire'&&e.ok)?.at});
  }

  const outOfOrderFrom=provider.ledger.length;provider.fault.delays=[500,0];
  const replies=await Promise.all([nativeCall('desktop','get_pr_detail',{repo:'synthetic-lab/repo-3',number:53}),nativeCall('paired','get_pr_detail',{repo:'synthetic-lab/repo-4',number:54})]);for(const r of replies)assert.equal(r.status,200);
  const receipts=provider.ledger.slice(outOfOrderFrom).filter(e=>e.bucket==='graphql'&&e.status===200);assert.ok(receipts.length>=2);assert.ok(receipts.some((a,i)=>receipts.slice(i+1).some(b=>a.at+a.elapsed>b.at+b.elapsed)),'provider replies completed out of request order');
  const beforeSnapshot=provider.ledger.length;const status=await control('status');assert.equal(provider.ledger.length,beforeSnapshot,'active admission snapshot adds no provider request');assert.ok(status.admission.graphql.remaining<=Math.min(...receipts.map(e=>e.remaining)),'late higher remaining cannot revive spent quota');
  result.phases.push({name:'real-out-of-order-replies-and-passive-admission-snapshot',applied:true,exercised:true,converged:true});
 }

 }

 if(mode==='contention'){
  const locker=spawn('python3',['scripts/enterprise/lock-profile.py',profile],{stdio:['pipe','pipe','pipe']});
  await new Promise((resolve,reject)=>{locker.stdout.once('data',data=>String(data).includes('LOCKED')?resolve():reject(Error('writer not held')));locker.once('error',reject);});
  try{
   const before=JSON.parse(execFileSync('python3',['scripts/enterprise/inspect-profile.py',profile],{encoding:'utf8'}));
   const started=performance.now();const blocked=await nativeCall('desktop','stats_board',{scopeKind:'repo',scopeValue:'synthetic-lab/repo-47',measure:'merged',days:1});
   const elapsedMs=performance.now()-started;assert.ok(!blocked.ok,'actual SQLite write contention must be observed');assert.ok(elapsedMs>=4500,'production five-second busy timeout was exercised');
   for(const page of pages)assert.equal(await page.getByTestId('counts').textContent(),'Authored 50 / Reviewing 150');
   const after=JSON.parse(execFileSync('python3',['scripts/enterprise/inspect-profile.py',profile],{encoding:'utf8'}));assert.deepEqual(after.queues,before.queues);assert.equal(after.integrity,'ok');
   result.phases.push({name:'actual-sqlite-writer-contention',applied:true,exercised:true,converged:false,elapsedMs});
  }finally{locker.stdin.end();await new Promise(r=>locker.once('exit',r));}
  const recovered=await nativeCall('desktop','stats_board',{scopeKind:'repo',scopeValue:'synthetic-lab/repo-47',measure:'merged',days:1});assert.equal(recovered.status,200);result.phases.at(-1).converged=true;
 }
 if(mode==='soak'||mode==='load'){
  provider.fault.historyPartial=false;
  await pages[0].getByRole('button',{name:'Back to list',exact:true}).click();
  await pages[0].getByRole('button',{name:/Synthetic review 53(?:\D|$)/}).first().click();
  await pages[0].getByText('Synthetic description 53',{exact:false}).waitFor();
  const observationStart=(await readFile(resolve(profile,'native.ndjson'),'utf8')).length;
  provider.fault.holdHistory=true;
  for(let n=0;n<150&&provider.heldCount===0;n++)await delay(1000);
  assert.ok(provider.heldCount>0,'actual historical background transport must be outstanding');
  const events=(await readFile(resolve(profile,'native.ndjson'),'utf8')).slice(observationStart).trim().split('\n').filter(Boolean).map(JSON.parse);
  const ticks=events.filter(e=>e.operation==='backfill-tick'&&e.stage==='begin');
  assert.ok(ticks.some(t=>!events.some(e=>e.id===t.id&&e.stage==='complete')),'production global backfill tick is active');
  assert.ok(provider.ledger.some(e=>e.operation==='history-search'&&e.held&&!e.released));
  provider.fault.delay=300;const started=performance.now();await pages[0].getByRole('button',{name:'Approve',exact:true}).first().click();
  await pages[0].getByRole('button',{name:'Working…',exact:true}).first().waitFor();
  const pendingMs=performance.now()-started;
  await pages[0].getByRole('button',{name:'Approved',exact:true}).first().waitFor();
  result.phases.push({name:'foreground-approval-during-held-production-backfill',applied:true,exercised:true,converged:true,pendingMs,confirmedMs:performance.now()-started});provider.fault.delay=0;provider.release();
 }
 if(mode==='tail')await runEnterpriseTail({pages,provider,result,nativeCall});
 if(mode==='soak')await runEnterpriseSoak({pages,provider,result,out,profile,nativeCall});
 }
 for(const [i,page] of pages.entries()) {
  const failed=await page.evaluate(()=>window.__enterprise.telemetry.filter(e=>e.kind==='call'&&e.ok===false&&e.name!=='diag_log'));
  if(mode==='offline')result.expectedFailures.push(...failed.filter(e=>['get_viewer','get_pr_detail','get_reviewing','refresh_now','get_review_gates'].includes(e.name)));
  result.errors.push(...failed.filter(e=>!result.expectedFailures.some(expected=>expected.name===e.name&&expected.at===e.at)).map(e=>({role:i?'paired':'desktop',command:e.name})));
 }
 for(const [i,page] of pages.entries()){assert.equal(await page.evaluate(()=>window.__enterprise.measurement.lost),false,'bounded browser telemetry intact');const metrics=await page.evaluate(()=>({uiCommits:window.__enterprise.commits.length,uiCommitMs:window.__enterprise.commits.reduce((n,c)=>n+c.actual,0),longTaskCount:window.__enterprise.longTasks?.length??null,longTaskMs:window.__enterprise.longTasks?.reduce((n,t)=>n+t.duration,0)??null}));assert.ok(metrics.uiCommits>0,'profiling renderer must emit actual commits');Object.assign(result.samples[i],metrics);}
 result.pass=result.errors.length===0;
}catch(error){result.failure=error.message;process.exitCode=1;}
finally {
 closing=true;
 for(const [i,page] of pages.entries()){try{await writeFile(resolve(out,`ui-${i}.json`),JSON.stringify(await page.evaluate(()=>window.__enterprise)));await page.screenshot({path:resolve(out,`final-${i}.png`)});}catch{}}
 for(const [i,c] of contexts.entries()){await c.tracing.stop({path:resolve(out,`trace-${i}.zip`)});await c.close();}
 if(browser)await browser.close();provider.release();
 if(bridge&&!exit){await control('stop');await Promise.race([exited,delay(10000)]);}
 if(!exit){child.kill('SIGTERM');await Promise.race([exited,delay(2000)]);if(!exit){child.kill('SIGKILL');await exited;}}
 if(proxy){proxy.closeAllConnections();await new Promise(r=>proxy.close(r));}await provider.close();
 try{await copyFile(resolve(profile,'native.ndjson'),resolve(out,'native.ndjson'));if(mode==='crash'){result.shutdown={intentionalInterruption:true};assert.equal(exit.signal,'SIGKILL');}else{result.shutdown=JSON.parse(await readFile(resolve(profile,'shutdown.json'),'utf8'));assert.equal(result.shutdown.telemetryLost,false);assert.equal(exit.code,0);}}catch(e){result.pass=false;result.failure??=e.message;}
 try{const events=(await readFile(resolve(out,'native.ndjson'),'utf8')).trim().split('\n').map(JSON.parse);assert.equal(provider.lost,false,'bounded provider ledger intact');if(result.cachedReads){for(const read of result.cachedReads){const scope=events.filter(e=>e.id===read.callId&&e.operation==='command');assert.equal(scope.length,2);assert.equal(scope[0].operation,'command');assert.equal(scope[1].stage,'complete');read.nativeScope=scope;read.concurrentSubmissions=events.filter(e=>e.stage==='begin'&&e.operation==='read-submitted'&&e.ns>=scope[0].ns&&e.ns<=scope[1].ns);}}result.accounting=reconcile(events,provider.ledger,{interrupted:mode==='crash'});if(mode==='crash')assert.ok(result.accounting.unfinished.some(e=>e.operation==='queue-transaction'));await writeFile(resolve(out,'profile-summary.json'),execFileSync('python3',['scripts/enterprise/inspect-profile.py',profile]));}catch(e){result.pass=false;result.failure??=e.message;}
 result.exit=exit;result.providerReceipts=provider.ledger.length;
 if(mode==='baseline'&&process.env.ENTERPRISE_BASELINE){try{result.baselineComparison=compareBaseline(result,JSON.parse(await readFile(process.env.ENTERPRISE_BASELINE,'utf8')));}catch(e){result.pass=false;result.failure??=e.message;}}
 await writeFile(resolve(out,'provider.json'),JSON.stringify(provider.ledger,null,2));await writeFile(resolve(out,'result.json'),JSON.stringify(result,null,2));console.log(JSON.stringify(result));if(!result.pass)process.exitCode=1;
}
