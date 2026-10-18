import assert from 'node:assert/strict';
export async function executeMixedPhase({record,requestedMs,startBoundary,endBoundary,work,observe,waitUntil,now=()=>performance.now()}){
 const boundaryStarted=now();await startBoundary();const start=now();Object.assign(record,{startedMs:start,requestedMs:requestedMs??null,startBoundaryMs:start-boundaryStarted});
 try{await work();if(requestedMs!==undefined){assert.ok(now()-start<=requestedMs,'mixed obligations exceeded fixed phase duration');await waitUntil({deadline:start+requestedMs,observe});}else await waitUntil({deadline:Math.max(start+5000,now()),observe});record.pass=true;}
 finally{record.elapsedMs=now()-start;const endStarted=now();await endBoundary();record.endBoundaryMs=now()-endStarted;}
}
