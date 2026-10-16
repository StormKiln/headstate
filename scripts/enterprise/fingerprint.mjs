import {createHash} from 'node:crypto';
import {spawn} from 'node:child_process';
import {createReadStream} from 'node:fs';
import {resolve} from 'node:path';
import {StringDecoder} from 'node:string_decoder';

// Stream exact stdout bytes; never retain or print patch/file contents. A
// failed command cannot publish a digest of its partial output.
async function* gitOutput(args,cwd) {
 const child=spawn('git',args,{cwd,stdio:['ignore','pipe','ignore']});
 const ended=new Promise(resolve=>{
  child.once('error',()=>resolve({spawnFailed:true}));
  child.once('close',(code,signal)=>resolve({code,signal}));
 });
 try {
  for await(const chunk of child.stdout)yield chunk;
  const result=await ended;
  if(result.spawnFailed)throw Error(`Could not start git ${args[0]} for enterprise fingerprint`);
  if(result.code!==0)throw Error(`git ${args[0]} failed for enterprise fingerprint (${result.signal?'signal '+result.signal:'exit '+result.code})`);
 }finally{
  if(child.exitCode===null&&child.signalCode===null)child.kill();
  await ended;
 }
}

async function* untrackedPaths(cwd) {
 const decoder=new StringDecoder('utf8');let pending='';
 for await(const chunk of gitOutput(['ls-files','--others','--exclude-standard'],cwd)){
  pending+=decoder.write(chunk);
  let end;
  while((end=pending.indexOf('\n'))!==-1){
   const path=pending.slice(0,end);pending=pending.slice(end+1);
   if(path)yield path;
  }
 }
 pending+=decoder.end();if(pending)yield pending;
}

export async function sourceFingerprint(cwd=process.cwd()) {
 const hash=createHash('sha256');
 for await(const chunk of gitOutput(['diff','HEAD','--no-ext-diff'],cwd))hash.update(chunk);
 // Preserve the existing Git listing order and path-then-content algorithm.
 // Backpressure bounds both Git output and each file to stream-sized chunks.
 for await(const path of untrackedPaths(cwd)){
  hash.update(path);
  for await(const chunk of createReadStream(resolve(cwd,path)))hash.update(chunk);
 }
 return hash.digest('hex');
}
