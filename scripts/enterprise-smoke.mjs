// Minimal native feasibility gate, deliberately separate from enterprise acceptance.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { createWriteStream } from 'node:fs';
import { resolve } from 'node:path';
import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { chromium, webkit } from 'playwright';
const out=resolve(process.argv[2]); await mkdir(out,{recursive:true});
const profile=resolve(out,'profile'); await mkdir(profile,{recursive:true});
const ready=resolve(out,'ready.json');
const secret=randomBytes(32).toString('hex');
let receipts=0;
const provider=createServer(async(req,res)=>{
  for await(const _chunk of req) { /* synthetic input not exported */ }
  receipts++;
  res.writeHead(200,{'content-type':'application/json'});
  res.end(JSON.stringify({data:{viewer:{login:'synthetic-viewer'},rateLimit:{remaining:5000,cost:1,resetAt:new Date(Date.now()+3600000).toISOString()}}}));
});
await new Promise(r=>provider.listen(0,'127.0.0.1',r));
const config=resolve(out,'private-config.json');
await writeFile(config,JSON.stringify({profile,provider:`http://127.0.0.1:${provider.address().port}`,bridge_secret:secret,ready}),{mode:0o600});
const binary=process.env.ENTERPRISE_DRIVER || resolve('src-tauri/target/debug/enterprise-driver');
const child=spawn(binary,[config],{stdio:['ignore','pipe','pipe']});
child.stdout.pipe(createWriteStream(resolve(out,'native.stdout')));child.stderr.pipe(createWriteStream(resolve(out,'native.stderr')));
let exit;const exited=new Promise(r=>child.once('exit',(code,signal)=>{exit={code,signal};r(exit);}));
let bridge,proxy;const browsers=[];const result={scope:'minimal real Wry, mounted browser, authenticated paired command smoke only',receipts:0,engines:[]};
const delay=ms=>new Promise(r=>setTimeout(r,ms));
try {
  for(let n=0;n<300;n++) {if(exit)throw Error('native exited before ready');try{bridge=JSON.parse(await readFile(ready,'utf8')).bridge;break;}catch{} await delay(100);}
  assert.ok(bridge,'native ready within 30s');
  assert.equal((await fetch(`${bridge}/control/status`,{method:'POST'})).status,401);
  proxy=createServer(async(req,res)=>{
    if(new URL(req.url,'http://127.0.0.1').pathname==='/') {res.writeHead(200,{'content-type':'text/html'});res.end('<!doctype html><title>Native synthetic smoke</title><main><button>Read native auth</button><pre></pre></main><script>document.querySelector("button").onclick=async()=>{const role=new URL(location).searchParams.get("role");const r=await fetch("/bridge/call/"+role+"/get_auth_state",{method:"POST",headers:{"Content-Type":"application/json"},body:"{}"});document.querySelector("pre").textContent=JSON.stringify(await r.json());};</script>');return;}
    if(!req.url.startsWith('/bridge/')){res.writeHead(404).end();return;}
    let body='';for await(const chunk of req)body+=chunk;
    const upstream=await fetch(bridge+req.url.slice(7),{method:req.method,headers:{'x-enterprise-secret':secret,'content-type':'application/json'},...(req.method==='POST'?{body}: {})});
    res.writeHead(upstream.status,{'content-type':'application/json'});res.end(await upstream.text());
  });
  await new Promise(r=>proxy.listen(0,'127.0.0.1',r));
  for(const [name,engine] of [['chromium',chromium],['webkit',webkit]]) {
    const browser=await engine.launch({headless:true});browsers.push(browser);
    const contexts=await Promise.all([browser.newContext(),browser.newContext()]);
    for(const [index,role] of ['desktop','paired'].entries()){
      const page=await contexts[index].newPage();page.setDefaultTimeout(10000);await page.goto(`http://127.0.0.1:${proxy.address().port}/?role=${role}`);await page.getByRole('button').click();
      await page.waitForFunction(()=>document.querySelector('pre').textContent.includes('synthetic account'));
      await page.screenshot({path:resolve(out,`${name}-${role}.png`)});
      const actual=JSON.parse(await page.locator('pre').textContent());assert.equal((actual.value??actual.wire).ok,true);
    }
    result.engines.push({name,version:browser.version(),roles:2});
    for(const context of contexts)await context.close();
  }
  const revoke=await fetch(`${bridge}/control/revoke`,{method:'POST',headers:{'x-enterprise-secret':secret}});assert.equal(revoke.status,200);
  const denied=await fetch(`${bridge}/call/paired/get_auth_state`,{method:'POST',headers:{'x-enterprise-secret':secret,'content-type':'application/json'},body:'{}'});assert.notEqual(denied.status,200);
  await fetch(`${bridge}/control/stop`,{method:'POST',headers:{'x-enterprise-secret':secret}});
  await Promise.race([exited,delay(10000).then(()=>{throw Error('unclean shutdown');})]);assert.equal(exit.code,0);
  result.shutdown=JSON.parse(await readFile(resolve(profile,'shutdown.json'),'utf8'));assert.equal(result.shutdown.clean,true);result.pass=true;
} catch(error) {result.pass=false;result.failure=error.message;process.exitCode=1;}
finally {
  for(const b of browsers)await b.close();
  if(!exit){child.kill('SIGTERM');await Promise.race([exited,delay(2000)]);if(!exit){child.kill('SIGKILL');await exited;}}
  if(proxy)await new Promise(r=>proxy.close(r));await new Promise(r=>provider.close(r));
  result.receipts=receipts;result.exit=exit;await writeFile(resolve(out,'result.json'),JSON.stringify(result,null,2));
  console.log(JSON.stringify(result));
}
