import assert from 'node:assert/strict';

// Exactly the three reads issued by PrDetailView.refreshDetail; new commands fail closed.
export const revocationReadCommands=Object.freeze(['get_pr_detail','get_review_gates','get_ready_stacks']);
// Only deliberately revoked paired Refresh reads can be exempted.
// Native command IDs connect browser failures to actual refused dispatches.
export function expectedRevocationFailures({window,responses,native,failures,continuity}) {
 assert.equal(window?.confirmed,true,'revocation must be confirmed');
 assert.ok(Number.isFinite(window.start)&&Number.isFinite(window.end)&&window.end>=window.start&&window.end-window.start<=30000,'finite 30-second revocation window');
 for(const key of ['before','during','after','repaired','errorCleared'])assert.equal(continuity?.[key],true,'revocation continuity '+key);
 const matched=failures.filter(call=>call.role==='paired'&&call.kind==='call'&&revocationReadCommands.includes(call.name)&&call.ok===false&&Number.isSafeInteger(call.callId)&&call.callId>0&&responses.some(response=>{
  if(response.role!=='paired'||response.command!==call.name||response.callId!==call.callId||response.status!==502||response.error!=='paired transport closed')return false;
  if(!Number.isFinite(response.started)||!Number.isFinite(response.completed)||response.started<window.start||response.completed<response.started||response.completed>window.end)return false;
  const lifecycle=native.filter(row=>row.operation==='command'&&row.id===call.callId);
  const begun=lifecycle.filter(row=>row.stage==='begin'&&row.code===1),refused=lifecycle.filter(row=>row.stage==='refused'&&row.code===0);
  return lifecycle.length===2&&begun.length===1&&refused.length===1&&Number.isFinite(begun[0].ns)&&Number.isFinite(refused[0].ns)&&refused[0].ns>=begun[0].ns;
 }));
 assert.ok(matched.some(call=>call.name==='get_pr_detail'),'deliberate UI refresh must produce a correlated revoked detail refusal');
 return matched;
}
