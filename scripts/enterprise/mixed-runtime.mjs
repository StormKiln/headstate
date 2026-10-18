import {readFile} from 'node:fs/promises';import {createHash} from 'node:crypto';
export async function nodeRuntimeIdentity(){return {version:process.version,modules:process.versions.modules,executableSha256:createHash('sha256').update(await readFile(process.execPath)).digest('hex')};}
// A cleared losing timeout must not retain the child orchestrator after shutdown.
export async function waitForProcess(exited,timeoutMs){let timer;try{return await Promise.race([exited.then(()=>false),new Promise(resolve=>{timer=setTimeout(()=>resolve(true),timeoutMs);})]);}finally{clearTimeout(timer);}}
