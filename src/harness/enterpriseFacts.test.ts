import {expect,test} from 'vitest';
import {churnRowFacts} from './enterpriseFacts';
import type {PullRequest} from '../types/pr';
const row={repo:'synthetic-lab/repo-1',number:51,author:'other',head_oid:'head-51',is_draft:false,in_merge_queue:false,requested_reviewers:['synthetic-viewer'],review:'none',latest_reviews:[{author:'synthetic-viewer',state:'APPROVED',commit_oid:'head-51'}]} as PullRequest;
test('read-only churn witness measures viewer current-head approval rather than aggregate review decision',()=>{expect(churnRowFacts(row).approved).toBe(true);expect(churnRowFacts({...row,head_oid:'new-head'}).approved).toBe(false);expect(churnRowFacts({...row,review:'approved',latest_reviews:[]}).approved).toBe(false);});

test('other-user conflicting review cannot erase or manufacture viewer approval',()=>{expect(churnRowFacts({...row,review:'changes_requested',latest_reviews:[...row.latest_reviews,{author:'other',state:'CHANGES_REQUESTED',commit_oid:'head-51'}]}).approved).toBe(true);expect(churnRowFacts({...row,latest_reviews:[{author:'other',state:'APPROVED',commit_oid:'head-51'}]}).approved).toBe(false);});
