import assert from 'node:assert/strict';
const normal=Array.from({length:128},(_,i)=>`session-${String(i).padStart(3,'0')}`);
export function corpusAuthority(status){const s=status.stored;return {paused:s.paused,sessions:s.sessions,usage:s.usage,indexedSources:s.indexedSources,healthyAuthority:s.healthyAuthority,linkAuthority:s.linkAuthority};}
export function assertMixedCorpus(status,manifest,{appended=false,linksAppended=appended,complete=false}={}){
 const s=status.stored,allowed=[...normal,'large','malformed','unreadable'];
 const ids=s.sessions.map(r=>r[0]);assert.equal(new Set(ids).size,ids.length);assert.ok(ids.every(id=>allowed.includes(id)),'no foreign generated sessions');
 for(const id of [...normal,'large','malformed'])assert.ok(ids.includes(id),`actual session ${id}`);
 if(complete)assert.deepEqual([...ids].sort(),[...allowed].sort(),'exact final generated corpus membership');
 const malformed=s.sessions.find(r=>r[0]==='malformed');assert.deepEqual(malformed.slice(1),[null,null],'malformed body does not invent cwd/prompt facts');
 const indexed=s.indexedSources.map(r=>r[0]);for(const id of [...normal,'large'])assert.ok(indexed.includes(id),`actual indexed source ${id}`);
 assert.equal(s.indexedSources.find(r=>r[0]==='large')[2],1,'actual bounded large-source truncation qualification');
 if(complete)assert.deepEqual([...indexed].sort(),[...allowed].sort(),'exact final indexed corpus membership');
 const input=s.usage.reduce((n,r)=>n+r[2],0),output=s.usage.reduce((n,r)=>n+r[3],0);
 assert.equal(input,manifest.expected[appended?'appendedInputTokens':'inputTokens'],'independent input token sum');assert.equal(output,manifest.expected[appended?'appendedOutputTokens':'outputTokens'],'independent output token sum');
 const links=Array.from({length:128},(_,i)=>i).filter(i=>linksAppended||i!==1).map(i=>[`session-${String(i).padStart(3,'0')}`,`synthetic-lab/repo-${i%50+1}`,51+i,`https://github.com/synthetic-lab/repo-${i%50+1}/pull/${51+i}`]);
 assert.deepEqual(s.linkAuthority.links.map(r=>r.slice(0,4)),links,'exact independently declared PR links');
 return {sessions:[...ids].sort(),indexed:[...indexed].sort(),links,inputTokens:input,outputTokens:output,largeTruncated:true,malformed:{cwd:null,prompt:null}};
}
export function assertMixedRollback(before,after){
 assert.ok(after.lastPass?.linkErrors?.some(error=>error.includes('enterprise_mixed_declared_link_rollback')),'actual fixed-source publication fault receipt required');
 assert.deepEqual(after.stored.linkAuthority,before.stored.linkAuthority,'full prior link/checkpoint/byte/digest authority retained');
 const a=before.stored.healthyAuthority[0],b=after.stored.healthyAuthority[0];assert.equal(a[0],b[0]);assert.notEqual(a[1],b[1],'metadata activity advances');assert.ok(b[2]>a[2],'index source bytes advance');assert.ok(b[4]>a[4],'usage source bytes advance');
}

export function assertMixedUnreadable(status){
 const indexed=status.lastPass?.indexed;
 assert.ok([...(indexed?.unreadable??[]),...(indexed?.scan_unreadable??[])].some(path=>path.includes('/synthetic/unreadable.jsonl:')&&/Permission denied/.test(path)),'actual declared-source native permission read failure');
}
