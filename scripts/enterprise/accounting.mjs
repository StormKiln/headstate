import assert from 'node:assert/strict';
export function reconcile(events, provider, {interrupted=false}={}) {
 const byId=new Map(),counts={};let reads=0,background=0,writes=0,maxReads=0,maxBackground=0,maxWrites=0;
 const admitted=new Map();
 const terminal=new Set(['cancelled','released','admitted','failed','body-complete','refused-body','receipt','committed','cas-rejected','complete','deadline','refused']);
 const seq=new Set();
 for(const e of events){
  assert.ok(!seq.has(e.seq),'unique recorder sequence');seq.add(e.seq);
  counts[`${e.operation}:${e.stage}`]=(counts[`${e.operation}:${e.stage}`]??0)+1;
  if(e.id){const rows=byId.get(e.id)??[];rows.push(e);byId.set(e.id,rows);}
  if(e.operation==='admitted'){
   if(e.stage==='begin'){admitted.set(e.id,e.code);if(e.code===3)writes++;else{reads++;if(e.code>0)background++;}}
   if(e.stage==='released'){const code=admitted.get(e.id);assert.notEqual(code,undefined);admitted.delete(e.id);if(code===3)writes--;else{reads--;if(code>0)background--;}}
   maxReads=Math.max(maxReads,reads);maxBackground=Math.max(maxBackground,background);maxWrites=Math.max(maxWrites,writes);
   assert.ok(reads>=0&&background>=0&&writes>=0);
  }
 }
 const unfinished=[];
 for(const [id,rows] of byId){const begins=rows.filter(e=>e.stage==='begin');assert.equal(begins.length,1,`scope ${id} begins once`);const ends=rows.filter(e=>terminal.has(e.stage)&&e.stage!=='begin');if(!ends.length)unfinished.push({id,operation:begins[0].operation});else assert.equal(ends.length,1,`scope ${id} ends once`);}
 assert.ok(maxReads<=4,'production four-read ceiling');assert.ok(maxBackground<=2,'production two-nonforeground ceiling');
 if(!interrupted){assert.equal(unfinished.length,0,'all native scopes terminal');assert.equal(admitted.size,0,'all admission permits released');}
 const submissions=events.filter(e=>e.stage==='begin'&&['read-submitted','write-submitted'].includes(e.operation));
 const aliases=submissions.reduce((n,e)=>n+e.code,0),providerAliases=provider.reduce((n,e)=>n+e.aliases,0);
 assert.ok(provider.length<=submissions.length,'provider receipts never inferred from Spend');
 assert.ok(providerAliases<=aliases,'provider AST aliases bounded by submitted structural aliases');
 for(const e of provider)assert.notEqual(e.operation,'unknown','every provider receipt classified');
 return {counts,maxReads,maxBackground,maxWrites,submissions:submissions.length,providerReceipts:provider.length,submittedAliases:aliases,providerAliases,unfinished,admittedRemaining:admitted.size};
}
