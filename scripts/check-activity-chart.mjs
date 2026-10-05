import assert from 'node:assert/strict';
import {preview} from 'vite';
import {chromium,webkit} from 'playwright';
import {mkdir,writeFile,readdir} from 'node:fs/promises';
import {resolve} from 'node:path';
const out=resolve(process.argv[2]);await mkdir(out,{recursive:true});assert.equal((await readdir(out)).length,0);
const engine=process.argv[3]??'chromium';
const server=await preview({configFile:'vite.harness.config.ts',preview:{host:'127.0.0.1',port:0}});
const browser=await ({chromium,webkit})[engine].launch({headless:true});const result={engine,version:browser.version(),pass:false,errors:[]};
const page=await browser.newPage({viewport:{width:1100,height:600},reducedMotion:'no-preference'});
page.on('pageerror',e=>result.errors.push(e.message));
await page.context().tracing.start({screenshots:true,snapshots:true});
try{
 await page.addInitScript(()=>{window.chartFrames=[];window.chartFrameLoss=false;const native=window.requestAnimationFrame;window.requestAnimationFrame=cb=>native.call(window,t=>{if(window.chartFrames.length<10000)window.chartFrames.push(performance.now());else window.chartFrameLoss=true;cb(t);});});
 await page.goto(server.resolvedUrls.local[0]+'harness/activity-chart.html');
 await page.locator('.recharts-area path').first().waitFor();await page.waitForTimeout(5000);
 const paths=()=>page.locator('.recharts-area path').evaluateAll(nodes=>nodes.map(n=>n.getAttribute('d')));
 const before=await paths();assert.equal(before.length,4);
 async function settled(){const count=await page.evaluate(()=>window.chartFrames.length);const countdown=await page.getByTestId('countdown').textContent();await page.waitForTimeout(3100);assert.notEqual(await page.getByTestId('countdown').textContent(),countdown,'production countdown continues');return await page.evaluate(n=>window.chartFrames.length-n,count);}
 result.unchangedFrames=await settled();assert.ok(result.unchangedFrames<10,'unchanged countdown must not restart chart animation');
 await page.getByRole('button',{name:'Change measurements'}).click();await page.getByRole('button',{name:'14d',exact:true}).click();
 await page.waitForTimeout(2500);assert.notDeepEqual(await paths(),before,'actual changed measurements update geometry');assert.equal(await page.getByRole('button',{name:'14d',exact:true}).getAttribute('aria-pressed'),'true');
 result.afterUpdateFrames=await settled();assert.ok(result.afterUpdateFrames<10,'real update animation settles with countdown still active');
 assert.equal(await page.evaluate(()=>window.chartFrameLoss),false);assert.deepEqual(result.errors,[]);result.pass=true;
}catch(e){result.failure=e.message;process.exitCode=1;}
finally{await page.screenshot({path:resolve(out,'chart.png')});await page.context().tracing.stop({path:resolve(out,'trace.zip')});await browser.close();await new Promise(r=>server.httpServer.close(r));await writeFile(resolve(out,'result.json'),JSON.stringify(result,null,2));console.log(JSON.stringify(result));}
