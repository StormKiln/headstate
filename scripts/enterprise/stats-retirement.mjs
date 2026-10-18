import assert from 'node:assert/strict';

export function validateEnterpriseMode(mode) {
 const modes=['quota','gate','authored','fault','retirement','crash','offline','load','baseline','soak','tail','tail-warm','convergence','convergence-restart','ready-progress','ready-completion','ready-dual-desktop','ready-dual-paired','ready-dual-lifecycle-desktop','ready-dual-lifecycle-paired'];
 assert.ok(modes.includes(mode),`Unsupported enterprise mode: ${mode}. Stats-only convergence and writer-contention modes are retired.`);
}

// Keep exact parity with the native remote-surface retirement contract.
export const retiredStatsCommands=new Set([
 'get_stats','get_periods','get_history','get_cycle_trend','get_merged_detail',
 'stats_count','stats_tree','stats_board','stats_board_cached','stats_demand',
 'stats_series','stats_reviewers','gitlab_stats_tree','gitlab_stats_load','gitlab_stats_backfill',
]);

// Classify received documents before fault handling so failed historical
// requests cannot disappear from the zero-request assertion. Parsed arguments
// include variables and aliases; ordinary current-PR detail and mutations stay
// distinct from closed-window aggregate/history searches.
export function statsRequestReasons(document,queryName,requestPath) {
 const reasons=[];
 if(requestPath){
  const url=new URL(requestPath,'http://synthetic.invalid');
  if(url.pathname.endsWith('/merge_requests')&&(
   (url.searchParams.get('state')==='all'&&url.searchParams.get('order_by')==='created_at')||
   (url.searchParams.get('state')==='merged'&&url.searchParams.get('order_by')==='merged_at')
  ))reasons.push('GitLab historical cohort request');
 }
 if(!document||document.kind==='mutation')return reasons;
 if(/stats|backfill|history/i.test(queryName??''))reasons.push('retired query name');
 for(const field of document.selection){
  if(field.name==='search'&&/(?:^|\s)(?:is:(?:merged|closed)|(?:merged|closed|created):)/.test(String(field.args.query??'')))reasons.push('historical or closed-window search');
 }
 const visit=fields=>{for(const field of fields){
  const states=[field.args.states].flat();
  if(field.name==='pullRequests'&&states.includes('MERGED')&&!Object.hasOwn(field.args,'headRefName'))reasons.push('merged repository history connection');
  visit(field.selection??[]);
 }};
 visit(document.selection);
 return [...new Set(reasons)];
}

export function statsRequestCounts(ledger,commands=[]) {
 return {providerRequests:ledger.filter(entry=>entry.statsRequestReasons?.length).length,commands:commands.filter(command=>retiredStatsCommands.has(command)).length,providerReceipts:ledger.length,meaning:'received documents classified before faults plus actual native bridge command names; no query-count inference from UI absence'};
}

export function assertNoStatsRequests(ledger,commands=[]) {
 const counts=statsRequestCounts(ledger,commands);
 assert.equal(counts.providerRequests,0,'enterprise run must receive zero retired Stats documents');
 assert.equal(counts.commands,0,'enterprise run must invoke zero retired Stats commands');
 return counts;
}
