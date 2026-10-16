import assert from 'node:assert/strict';
import test from 'node:test';
import {mkdtemp,writeFile,readFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {sourceFingerprint} from './fingerprint.mjs';

const git=(cwd,args,options={})=>execFileSync('git',args,{cwd,env:{...process.env,GIT_AUTHOR_NAME:'Synthetic',GIT_AUTHOR_EMAIL:'synthetic@invalid',GIT_COMMITTER_NAME:'Synthetic',GIT_COMMITTER_EMAIL:'synthetic@invalid'},...options});

test('patch larger than default exec buffer and large untracked files preserve exact legacy digest and order',async()=>{
 const cwd=await mkdtemp(join(tmpdir(),'enterprise-fingerprint-'));
 try{
  git(cwd,['init','-q']);await writeFile(join(cwd,'tracked.txt'),'initial\n');
  git(cwd,['add','tracked.txt']);git(cwd,['commit','-qm','synthetic fixture']);
  await writeFile(join(cwd,'tracked.txt'),'synthetic large patch row\n'.repeat(90000));
  await writeFile(join(cwd,'a-first.bin'),Buffer.alloc(2*1024*1024,0xab));
  await writeFile(join(cwd,'z-last.txt'),'last synthetic file\n');
  const diff=git(cwd,['diff','HEAD','--no-ext-diff'],{maxBuffer:8*1024*1024});
  assert.ok(diff.length>1024*1024,'the real Git patch must exceed the old default buffer');
  const expected=createHash('sha256').update(diff);
  const files=git(cwd,['ls-files','--others','--exclude-standard'],{encoding:'utf8'}).trim().split('\n').filter(Boolean);
  assert.deepEqual(files,['a-first.bin','z-last.txt']);
  for(const file of files)expected.update(file).update(await readFile(join(cwd,file)));
  assert.equal(await sourceFingerprint(cwd),expected.digest('hex'));
 }finally{await rm(cwd,{recursive:true,force:true});}
});

test('Git failure rejects instead of returning a valid-looking hash or leaking stderr',async()=>{
 const cwd=await mkdtemp(join(tmpdir(),'enterprise-fingerprint-failure-'));
 try{
  // Unborn HEAD makes the production git diff fail even inside a repository.
  git(cwd,['init','-q']);
  await assert.rejects(sourceFingerprint(cwd),error=>{
   assert.match(error.message,/git diff failed for enterprise fingerprint \(exit 128\)/);
   assert.ok(!error.message.includes(cwd));return true;
  });
 }finally{await rm(cwd,{recursive:true,force:true});}
});
