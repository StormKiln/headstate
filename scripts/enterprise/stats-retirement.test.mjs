import assert from 'node:assert/strict';
import test from 'node:test';
import {readFile} from 'node:fs/promises';
import {parseDocument} from './graphql.mjs';
import {assertNoStatsRequests,statsRequestCounts,statsRequestReasons,retiredStatsCommands,validateEnterpriseMode} from './stats-retirement.mjs';
import {startProvider,fixture} from './provider.mjs';

test('historical aliases and variables are classified while active review reads and mutations remain allowed',()=>{
 const classify=(query,variables={})=>statsRequestReasons(parseDocument(query,variables),query.match(/query\s+(\w+)/)?.[1]);
 assert.deepEqual(classify('query { search(query:"is:pr is:open review-requested:@me"){nodes{id}} }'),[]);
 assert.deepEqual(classify('query { repository(owner:"synthetic-lab",name:"repo-1"){pullRequest(number:51){reviews{nodes{id}}}} }'),[]);
 assert.deepEqual(classify('mutation { mergePullRequest(input:{pullRequestId:"PR_51"}){pullRequest{mergedAt}} }'),[]);
 assert.ok(classify('query($q:String!){a:search(query:$q){issueCount}}',{q:'is:pr created:2026-01-01..2026-01-31'}).length);
 assert.ok(classify('query StatsViewer {viewer{login}}').length);
 assert.throws(()=>assertNoStatsRequests([{statsRequestReasons:['historical search']}],[]));
 assert.throws(()=>assertNoStatsRequests([],['stats_board_cached']));
 assert.equal(assertNoStatsRequests([{statsRequestReasons:[]}],['get_pr_detail']).providerRequests,0);
});

test('failed historical requests are counted and retained fixture populations are unchanged',async()=>{
 const provider=await startProvider();try{
  provider.fault.mode='secondary';
  const response=await fetch(provider.url+'/graphql',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({query:'query {search(query:"is:pr is:merged"){nodes{id}}}'})});
  assert.equal(response.status,429);assert.equal(provider.ledger[0].statsRequestReasons.length,1);
  assert.throws(()=>assertNoStatsRequests(provider.ledger));
  assert.equal(fixture().rows.filter(row=>row.state==='MERGED').length,250);
  assert.equal(fixture().rows.filter(row=>row.state==='OPEN').length,200);
 }finally{await provider.close();}
});

test('active runner has no retired commands or mount actions and retired modes fail explicitly',async()=>{
 validateEnterpriseMode('fault');validateEnterpriseMode('convergence');
 assert.throws(()=>validateEnterpriseMode('stats-convergence'));assert.throws(()=>validateEnterpriseMode('contention'));
 for(const file of ['../enterprise-run.mjs','final-acceptance.mjs','warm-tail.mjs','ready-completion.mjs']){
  const source=await readFile(new URL(file,import.meta.url),'utf8');
  assert.ok(!/['"]stats_(?:board|board_cached|demand)['"]|name:['"]Statistics['"]/.test(source),file);
 }
});


const retiredCommands=['get_stats','get_periods','get_history','get_cycle_trend','get_merged_detail','stats_count','stats_tree','stats_board','stats_board_cached','stats_demand','stats_series','stats_reviewers','gitlab_stats_tree','gitlab_stats_load','gitlab_stats_backfill'];
for(const command of retiredCommands)test(`rejected ${command} invocation still counts with no provider receipt`,()=>{
 assert.equal(statsRequestCounts([],[command]).commands,1);
 assert.throws(()=>assertNoStatsRequests([],[command]));
});
test('retired command set matches the complete native remote-surface contract',async()=>{
 const source=await readFile(new URL('../../src-tauri/src/remote/surface.rs',import.meta.url),'utf8');
 const contract=source.split('fn retired_pr_stats_commands_are_not_remote_capabilities()')[1].split('] {')[0];
 const names=[...contract.matchAll(/"([a-z_]+)"/g)].map(match=>match[1]);
 assert.deepEqual([...retiredStatsCommands].sort(),names.sort());
 assert.deepEqual([...retiredStatsCommands].sort(),retiredCommands.sort());
});
const retiredDocuments=[
 ['weekly/monthly counts','query($week:String!,$month:String!){merged_week:search(query:$week){issueCount} merged_month:search(query:$month){issueCount}}',{week:'is:pr author:@me is:merged merged:2026-01-01..2026-01-07',month:'is:pr author:@me is:merged merged:2026-01-01..2026-01-31'}],
 ['opened period/history/trend','query {o0:search(query:"is:pr author:@me created:2026-01-01"){issueCount}}',{}],
 ['merged detail','query {merged:search(query:"is:pr author:@me is:merged"){nodes{number createdAt mergedAt}}}',{}],
 ['repository connection','query {repository(owner:"synthetic-lab",name:"repo-1"){pullRequests(states:MERGED,first:50,orderBy:{field:UPDATED_AT,direction:DESC}){nodes{number createdAt mergedAt}}}}',{}],
];
for(const [name,query,variables]of retiredDocuments)test(`provider counts retired ${name} document even when rejected`,async()=>{
 const provider=await startProvider();try{
  provider.fault.mode='secondary';
  const response=await fetch(provider.url+'/graphql',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({query,variables})});
  assert.equal(response.status,429);
  assert.equal(statsRequestCounts(provider.ledger).providerRequests,1);
  assert.throws(()=>assertNoStatsRequests(provider.ledger));
 }finally{await provider.close();}
});
for(const [state,order]of [['all','created_at'],['merged','merged_at']])test(`GitLab ${state} cohort requests count before rejection`,async()=>{
 const provider=await startProvider();try{
  provider.fault.mode='secondary';
  const response=await fetch(provider.url+`/api/v4/projects/synthetic%2Frepo/merge_requests?scope=all&state=${state}&order_by=${order}&sort=desc`);
  assert.equal(response.status,429);assert.equal(statsRequestCounts(provider.ledger).providerRequests,1);
  assert.throws(()=>assertNoStatsRequests(provider.ledger));
 }finally{await provider.close();}
});
test('live merged-head matching and GitLab open queues remain allowed',()=>{
 const doc=parseDocument('query {repository(owner:"synthetic-lab",name:"repo-1"){h0:pullRequests(headRefName:"topic-51",states:MERGED,first:10){nodes{number}}}}');
 assert.deepEqual(statsRequestReasons(doc),[]);
 assert.deepEqual(statsRequestReasons(null,undefined,'/api/v4/merge_requests?state=opened&scope=assigned_to_me'),[]);
 assert.equal(statsRequestCounts([],['get_pr_detail','get_reviewing','get_ready_stacks']).commands,0);
});
