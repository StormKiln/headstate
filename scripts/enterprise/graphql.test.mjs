import { test } from 'node:test';import assert from 'node:assert/strict';import { parseDocument, project } from './graphql.mjs';
test('aliases exclude argument colons strings comments and rateLimit',()=>{
 const d=parseDocument('query($q:String!){rateLimit{remaining} a:search(query:$q){nodes{... on PullRequest{id}}} # fake: field\n b:search(query:"repo:synthetic/a"){issueCount}}',{q:'x:y'});
 assert.equal(d.aliases,2);assert.equal(d.selection[1].args.query,'x:y');assert.deepEqual(project(d.selection,{rateLimit:{remaining:0},search:{nodes:[{id:'PR_1'}],issueCount:1}},(f,o)=>o[f.name]),{rateLimit:{remaining:0},a:{nodes:[{id:'PR_1'}]},b:{issueCount:1}});
});
test('malformed and unsupported shapes fail explicitly',()=>{for(const q of ['query{a','query{a @skip(if:true)}','query{a} fragment X on Y{z}'])assert.throws(()=>parseDocument(q));});
