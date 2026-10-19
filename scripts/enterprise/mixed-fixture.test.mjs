import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp, mkdir, writeFile, readFile, symlink} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {generateMixedFixture, appendMixedFixture} from './mixed-fixture.mjs';
async function marked(){const profile=await mkdtemp(join(tmpdir(),'r109-mixed-fixture-'));await writeFile(join(profile,'.enterprise-synthetic'),'unit');const root=join(profile,'generated-corpus/projects');await mkdir(root,{recursive:true});return {profile,root};}
test('fixture retains over100 valid pending bodies, bounded large source and exact append plan',async()=>{
 const a=await marked(),b=await marked();const first=await generateMixedFixture(a),second=await generateMixedFixture(b);
 assert.equal(first.validSessions,129);assert.equal(first.files.length,131);assert.equal(first.logicalSha256,second.logicalSha256);assert.notDeepEqual(first.files,second.files,'actual cwd bytes remain rooted and honestly hashed');
 assert.ok(first.files.find(f=>f.kind==='large').bytes>8*1024*1024);
 const large=(await readFile(join(a.root,'synthetic/large.jsonl'),'utf8')).trim().split('\n');assert.ok(large.length>40);assert.ok(large.every(row=>Buffer.byteLength(row)<2048),'large valid source keeps every metadata record bounded');large.forEach(row=>JSON.parse(row));
 const body=await readFile(join(a.root,'synthetic/session-000.jsonl'),'utf8');assert.ok(body.includes(a.root));assert.ok(body.includes('mixedneedle000'));
 const before=await readFile(join(a.root,'synthetic/session-001.jsonl'),'utf8');const appended=await appendMixedFixture(a,first);assert.ok(appended.afterBytes>appended.beforeBytes);assert.ok((await readFile(join(a.root,'synthetic/session-001.jsonl'),'utf8')).startsWith(before));
 await assert.rejects(appendMixedFixture(a,first),/append source bytes/);await assert.rejects(generateMixedFixture(a),/empty/);
});
test('fixture rejects unmarked and escaped roots before writes',async()=>{
 const a=await marked(),outside=await mkdtemp(join(tmpdir(),'r109-outside-'));await assert.rejects(generateMixedFixture({profile:a.profile,root:outside}),/descendant/);
 await symlink(outside,join(a.profile,'generated-corpus/escape'));await assert.rejects(generateMixedFixture({profile:a.profile,root:join(a.profile,'generated-corpus/escape')}),/descendant/);
 const b=await mkdtemp(join(tmpdir(),'r109-unmarked-'));await assert.rejects(generateMixedFixture({profile:b,root:b}));
});
