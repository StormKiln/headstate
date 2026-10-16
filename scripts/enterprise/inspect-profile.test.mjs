import assert from 'node:assert/strict';
import test from 'node:test';
import {mkdtemp,writeFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {execFileSync} from 'node:child_process';

test('profile evidence reads queue authority without retired tables and preserves existing historical data',async()=>{
 const profile=await mkdtemp(join(tmpdir(),'enterprise-profile-contract-'));
 try{
  await writeFile(join(profile,'.enterprise-synthetic'),'synthetic');
  execFileSync('python3',['-c',`import sqlite3,sys,json
p=sqlite3.connect(sys.argv[1])
p.execute('CREATE TABLE queue_scan(list TEXT,revision INTEGER,payload TEXT)')
p.execute('INSERT INTO queue_scan VALUES(?,?,?)',('reviewing',7,json.dumps({'done':True,'seen':['PR_51'],'candidates':['PR_51']})))
p.execute('CREATE TABLE pr_history(retained TEXT)')
p.execute("INSERT INTO pr_history VALUES('keep synthetic history')")
p.commit()`,join(profile,'headstate.db')]);
  const result=JSON.parse(execFileSync('python3',[new URL('./inspect-profile.py',import.meta.url).pathname,profile],{encoding:'utf8'}));
  assert.equal(result.integrity,'ok');assert.deepEqual(result.counts,{queue_scan:1});
  assert.equal(result.queues[0].revision,7);assert.equal(result.queues[0].seen,1);
  const retained=execFileSync('python3',['-c','import sqlite3,sys;print(sqlite3.connect(sys.argv[1]).execute("SELECT retained FROM pr_history").fetchone()[0])',join(profile,'headstate.db')],{encoding:'utf8'}).trim();
  assert.equal(retained,'keep synthetic history');
 }finally{await rm(profile,{recursive:true,force:true});}
});
