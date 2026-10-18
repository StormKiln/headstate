import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {lstat, realpath, readdir, mkdir, writeFile, readFile, appendFile} from 'node:fs/promises';
import {resolve, relative, sep} from 'node:path';
const hash=bytes=>createHash('sha256').update(bytes).digest('hex');
const timestamp='2026-10-18T12:00:00Z';
async function validated({profile,root}){
 const base=await realpath(profile),marker=await lstat(resolve(base,'.enterprise-synthetic'));
 assert.ok(marker.isFile()&&!marker.isSymbolicLink(),'marked generated profile required');
 const generated=await realpath(resolve(base,'generated-corpus')),actual=await realpath(root);
 assert.ok(generated.startsWith(base+sep)&&actual.startsWith(generated+sep),'strict generated-root descendant required');
 assert.ok((await lstat(actual)).isDirectory(),'corpus root directory required');return actual;
}
const line=value=>JSON.stringify(value)+'\n';
function records(index,cwd){
 const id=`mixed-session-${String(index).padStart(3,'0')}`,tool=`mixed-tool-${index}`;
 const rows=[{type:'user',sessionId:id,timestamp,cwd,message:{role:'user',content:`mixedneedle${String(index).padStart(3,'0')} accepted synthetic prompt`}},
 {type:'assistant',sessionId:id,timestamp,cwd,message:{id:`usage-${index}`,role:'assistant',model:'claude-sonnet-4-20250514',usage:{input_tokens:11,output_tokens:7,cache_read_input_tokens:0,cache_creation_input_tokens:0},content:[{type:'tool_use',id:tool,name:'Bash',input:{command:'gh pr create --title synthetic'}}]}}];
 if(index!==1)rows.push(result(index,id,tool));return rows;
}
function result(index,id=`mixed-session-${String(index).padStart(3,'0')}`,tool=`mixed-tool-${index}`){const url=`https://github.com/synthetic-lab/repo-${index%50+1}/pull/${51+index}`;return {type:'user',sessionId:id,timestamp,message:{role:'user',content:[{type:'tool_result',tool_use_id:tool,content:url,is_error:false}]},toolUseResult:{stdout:url,stderr:'',interrupted:false}};}
export async function generateMixedFixture(options){
 const root=await validated(options);assert.deepEqual(await readdir(root),[],'fixture requires empty initialized root');
 const project=resolve(root,'synthetic');await mkdir(project);const files=[],logical=[];
 async function put(name,kind,template){const body=template.replaceAll('$CORPUS_PROJECT',project);await writeFile(resolve(project,name),body,{flag:'wx'});files.push({path:`synthetic/${name}`,kind,bytes:Buffer.byteLength(body),sha256:hash(body)});logical.push({path:`synthetic/${name}`,kind,bytes:Buffer.byteLength(template),sha256:hash(template)});}
 for(let n=0;n<128;n++)await put(`session-${String(n).padStart(3,'0')}.jsonl`,'valid',records(n,'$CORPUS_PROJECT').map(line).join(''));
 // An actual >8MiB source proves the production index truncation qualification.
 const large=line({type:'user',sessionId:'mixed-large',timestamp,cwd:'$CORPUS_PROJECT',message:{role:'user',content:'mixedlargeneedle'}})+line({type:'assistant',timestamp,message:{role:'assistant',content:'x'.repeat(1024)}}).repeat(9*1024);
 await put('large.jsonl','large',large);await put('malformed.jsonl','malformed','{intentionally invalid JSON\n');
 // Permissions are changed only inside the declared degradation phase and
 // restored before the immutable checkpoint hash. Failed attempts remain.
 await put('unreadable.jsonl','unreadable-candidate',line({type:'user',sessionId:'mixed-unreadable',timestamp,cwd:'$CORPUS_PROJECT',message:{role:'user',content:'mixed unreadable candidate'}}));
 return {schema:1,version:'mixed-corpus-v1',validSessions:129,files,logicalSha256:hash(JSON.stringify(logical)),append:{path:'synthetic/session-001.jsonl',beforeSha256:files.find(f=>f.path.endsWith('session-001.jsonl')).sha256,body:line(result(1))+line({type:'user',timestamp:'2026-10-18T12:01:00Z',message:{role:'user',content:'mixedappendneedle healthy indexed append'}})+line({type:'assistant',timestamp:'2026-10-18T12:01:01Z',message:{id:'usage-append',role:'assistant',model:'claude-sonnet-4-20250514',usage:{input_tokens:3,output_tokens:2},content:'synthetic append'}})},expected:{baselineNormalSessions:128,baselineLargeSessions:1,baselineLinks:127,appendedLinks:128,inputTokens:128*11,outputTokens:128*7,appendedInputTokens:128*11+3,appendedOutputTokens:128*7+2}};
}
export async function appendMixedFixture(options,manifest){
 const root=await validated(options),path=resolve(root,manifest.append.path);assert.ok(relative(root,path)&&!relative(root,path).startsWith('..'),'append containment');
 assert.ok(!(await lstat(path)).isSymbolicLink(),'append cannot follow symlink');const before=await readFile(path);assert.equal(hash(before),manifest.append.beforeSha256,'append source bytes must match initial fixture');
 await appendFile(path,manifest.append.body);const after=await readFile(path);return {path:manifest.append.path,beforeBytes:before.length,afterBytes:after.length,beforeSha256:hash(before),afterSha256:hash(after)};
}
