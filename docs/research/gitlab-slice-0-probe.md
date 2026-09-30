# GitLab slice 0: bounded GitLab.com probe

**Date:** 2026-09-23. **Host:** `gitlab.com`. **Disposable fixture:**
`<namespace>/<test-project>`. **Tool:** `glab 1.119.0`. Authenticated REST and
GraphQL reads used an explicit hostname and a 15-second request timeout.
This report stores only schema, counts, selected state values, and timing;
it stores no raw response body, token, person, or private project data.

## Fixture provenance

The project already had `main` and a README. Branch
`codex/slice0-read-fixture` began at `58d83a6` with the two-line
`headstate-slice0-fixture.txt`; **draft MR !1** targets `main`. Commit
`9aa7fc4` added one MR-only `.gitlab-ci.yml` job to that branch.
Branch `codex/slice0-page2-fixture` at `61a04b2` contains one one-line
text file; **draft MR !2** also targets `main`. MR !1 has one resolved
diff discussion on the fixture file. Both MRs remain open and draft.
No merge, deletion, reviewer assignment, or unrelated project resource
was created. The fixture has two authored MRs and no assigned reviewer.

## Observed REST response shapes

| Request | `glab api` | Direct OAuth Bearer HTTP | Redacted shape |
| --- | --- | --- | --- |
| `GET projects/:path` | exit 0 | 200 | Object; `id: int`, `path_with_namespace: string`, `default_branch: string`, `empty_repo: bool` |
| MR `state=all&per_page=1&page=1` before fixture | exit 0 | 200 | Empty array |
| Same request, page 2 before fixture | exit 0 | 200 | Empty array |
| Project pipelines before fixture | exit 0 | 200 | Empty array |
| Missing MR `!999999` | exit 1 | 404 | Object with `message: string`; `glab` also writes an error to stderr |
| MR `state=all&per_page=1&page=1` after fixture | exit 0 | 200 | One object |
| Same request, page 2 after fixture | exit 0 | 200 | Empty array; total remains 1 |
| MR `scope=created_by_me&state=opened` | exit 0 | 200 | One object |
| MR `scope=reviews_for_me&state=opened` | exit 0 | 200 | Empty array; no reviewer assigned |
| MR `!1` detail | exit 0 | 200 | Object |
| MR `!1` approvals | exit 0 | 200 | Object |
| MR `!1` approval state | exit 0 | 200 | Object with `rules: []` and `approval_rules_overwritten: false` |
| MR `!1` discussions | exit 0 | 200 | Empty array |
| MR `!1` pipelines | exit 0 | 200 | Empty array |
| MR `!1` commits | exit 0 | 200 | One object |

The populated MR list item has these keys (values omitted):
`approvals_before_merge`, `assignee`, `assignees`, `author`,
`blocking_discussions_resolved`, `closed_at`, `closed_by`, `created_at`,
`description`, `detailed_merge_status`, `discussion_locked`, `downvotes`,
`draft`, `force_remove_source_branch`, `has_conflicts`, `id`, `iid`,
`imported`, `imported_from`, `labels`, `merge_after`, `merge_commit_sha`,
`merge_status`, `merge_user`, `merge_when_pipeline_succeeds`, `merged_at`,
`merged_by`, `milestone`, `prepared_at`, `project_id`, `reference`,
`references`, `reviewers`, `sha`, `should_remove_source_branch`,
`source_branch`, `source_project_id`, `squash`, `squash_commit_sha`,
`squash_on_merge`, `state`, `target_branch`, `target_project_id`,
`task_completion_status`, `time_stats`, `title`, `updated_at`, `upvotes`,
`user_notes_count`, `web_url`, `work_in_progress`.

Detail adds `changes_count`, `diff_refs`, `first_contribution`,
`first_deployed_to_production_at`, `head_pipeline`,
`latest_build_finished_at`, `latest_build_started_at`, `merge_error`,
`pipeline`, `subscribed`, and `user`. Observed types: `iid` and `id` are
integers; `draft`, `has_conflicts`, and `blocking_discussions_resolved` are
booleans; `detailed_merge_status`, `merge_status`, and `changes_count`
are **strings**. `pipeline` and `head_pipeline` were **null**, while
`reviewers` was an empty array. `time_stats` contained integer
`time_estimate` and `total_time_spent`, plus nullable
`human_time_estimate` and `human_total_time_spent`.

The approvals object exposed `approval_rules_left`, `approvals_left`,
`approvals_required`, `approved`, `approved_by`, `approver_groups`,
`approvers`, `has_approval_rules`, `invalid_approvers_rules`,
`merge_request_approvers_available`, `multiple_approval_rules_available`,
`require_password_to_approve`, `suggested_approvers`, `user_can_approve`,
and `user_has_approved`, along with MR metadata. This fixture had
`approved: true`, `approvals_required: 0`, `approvals_left: 0`,
`approved_by: []`, `user_has_approved: false`, `user_can_approve: false`,
and `has_approval_rules: false`. Here `approved: true` means no approval
is outstanding; it is **not evidence of a reviewer approval**.

The draft detail had `state: opened`, `draft: true`,
`detailed_merge_status: draft_status`, `merge_status: can_be_merged`,
`has_conflicts: false`, `blocking_discussions_resolved: true`,
`changes_count: "1"`, and `user_notes_count: 0`. A UI must not turn the
coarse `merge_status` into a merge-ready claim while the detailed status
says draft. Null pipeline fields are not a green pipeline result.

The one commit item exposed `author_email`, `author_name`,
`authored_date`, `committed_date`, `committer_email`, `committer_name`,
`created_at`, `extended_trailers`, `id`, `message`, `parent_ids`,
`short_id`, `title`, `trailers`, and `web_url`. No values were retained.

## Pagination and GraphQL stats shape

For the populated REST MR list with `per_page=1`, direct HTTP returned
`X-Total: 1`, `X-Total-Pages: 1`, and `X-Page: 1` on page 1; page 2
returned an empty array with the same total and `X-Page: 2`.
`glab api -i` exposed header names `link`, `x-next-page`, `x-page`,
`x-per-page`, `x-prev-page`, `x-total`, `x-total-pages`,
`ratelimit-limit`, `ratelimit-remaining`, and `ratelimit-reset`.
Default `glab api` JSON output has no headers. For the empty reviewer
scope, direct HTTP returned `X-Total: 0` and `X-Total-Pages: 1`;
zero total and one page are different concepts.

A bounded GraphQL project `mergeRequests(first: 1)` query returned
`count: 1`, one node with `iid`, `state`, and `draft`, and `pageInfo`
with `hasNextPage: false` and `endCursor: null` in 603 ms through `glab`.
The same query with `state: merged` returned `count: 0`, no nodes, and
`hasNextPage: false` in 646 ms. Both returned `data` with no GraphQL
errors. This verifies a one-item and a zero-item connection shape, not
multi-page stats backfill or merged-MR metrics.

## Wall-clock latency

Milliseconds were measured around complete subprocess calls or Python
`urllib.request` calls, including response-body reads. Requests ran
sequentially on the same Mac and network, so these are exploratory timings.

| Request | `glab api` | Direct HTTP |
| --- | ---: | ---: |
| First project request, before direct auth correction | 669 | — |
| Project in first paired run | 647 | 443 |
| Empty MR list page 1 | 634 | 311 |
| Empty MR list page 2 | 588 | 258 |
| Empty project pipeline page | 634 | 247 |
| Missing MR detail | 572 | 281 |
| Three subsequent project reads | 890, 790, 652 (median 790) | 393, 368, 359 (median 368) |
| Populated MR list page 1 / page 2 | 774 / 689 | 305 / 363 |
| Authored / reviewer list | 666 / 639 | 303 / 271 |
| MR detail | 631 | 345 |
| Approvals / approval state | 671 / 571 | 314 / 371 |
| Discussions / MR pipelines | 678 / 651 | 272 / 385 |
| MR commits | 679 | 284 |

The first direct HTTP attempt used a `PRIVATE-TOKEN` header and returned
401. This login uses an OAuth credential; `Authorization: Bearer`
succeeded. That 401 was a probe setup error, not a finding about direct
HTTP viability. The credential was read into process memory from
`glab config get oauth_token --host gitlab.com` and never printed or
passed as a process argument. A production adapter must also handle
keychain storage and token refresh; this probe does not validate either.

For cancellation, a running read-only `glab api` subprocess was sent
SIGTERM after 10 and 50 ms. It exited with `-15` after 16 and 56 ms
total, respectively. The caller must still enforce timeouts and handle
cancellation. Direct HTTP cancellation was not tested.

## Extended fixture: two pages, discussion, and CI

With `state=opened&per_page=1`, REST page 1 returned MR !2, page 2
returned MR !1, and page 3 was empty. Direct HTTP headers on pages 1
and 2 reported `X-Total: 2` and `X-Total-Pages: 2`; page 1 had
`X-Next-Page: 2`. Page order is an observation, not an identity rule.
Wall-clock milliseconds: page 1 `glab` 719 / direct 375; page 2
892 / 392; page 3 629 / 350. The two populated REST pages cost
1.611 s through `glab` and 0.767 s through direct HTTP in this run.

A `mergeRequests(first: 1, after: $after)` GraphQL query returned
`count: 2`; page 1 had one node, `hasNextPage: true`, and a cursor;
page 2 had one node and `hasNextPage: false`. `glab` page timings were
669 and 641 ms (1.310 s total); direct OAuth Bearer timings were 330
and 286 ms (0.616 s total). Cursor values and raw nodes were not stored.

The discussion was created with form-encoded `position[...]` fields
using the newest MR version's base, start, and head SHA and an added line
in `headstate-slice0-fixture.txt`. The created note was a `DiffNote`
with `resolvable: true`, `resolved: false`. Resolving the thread returned
`resolved: true`; a later GET confirmed it. A JSON-body create attempt
had failed before this. Readback found no fixture comment from that
attempt. The documented form request succeeded. [GitLab's Discussions
API](https://docs.gitlab.com/api/discussions/) describes both forms of
thread creation and the resolution endpoint.

A bounded discussion read with `per_page=2` returned two entries on
page 1 and one on page 2 (`X-Total: 3`, `X-Total-Pages: 2`). Two were
non-resolvable system notes; the fixture diff thread had one note with
`type: DiffNote`, `resolvable: true`, `resolved: true`. The discussion
object has `id`, `individual_note`, `notes`, and `resolvable`; note-level
resolution must be read separately. Page 1 took 750 ms through `glab`
and 382 ms direct; page 2 took 609 ms through `glab`.

MR !1's pipeline list returned one object with `created_at`, `id`,
`iid`, `project_id`, `ref`, `sha`, `source`, `status`, `updated_at`,
and `web_url`; its status was `success`. The pipeline detail reported
`source: merge_request_event` and `status: success`; the jobs endpoint
had one job with `status: success` and `allow_failure: false`. The MR
detail's `pipeline` and `head_pipeline` changed from null to objects
with `status: success`. The pipeline list took 603 ms through `glab`
and 327 ms direct; pipeline detail and jobs took 586 and 764 ms through
`glab`. The job uses GitLab's documented MR pipeline rule and ran once.
[GitLab's MR pipeline guide](https://docs.gitlab.com/ci/pipelines/merge_request_pipelines/)
explains the `merge_request_event` rule.

Across adjacent detail reads, `detailed_merge_status` briefly appeared
as `checking` and then `draft_status`. Keep that state pending while
GitLab recalculates it; neither sample proves merge readiness.

## Provisional GitLab.com transport decision

**Use bounded `glab api --hostname` subprocesses for the initial
GitLab.com read adapter.** The two-page REST list took 1.611 s and the
two-page GraphQL connection took 1.310 s, inside Headstate's 30-second
per-fetch limit (`poll.rs` `FETCH_TIMEOUT`), 45-second poll tick limit
(`TICK_TIMEOUT`), and 60-second stats load limit (`stats/fetch.rs`
`LOAD_TIMEOUT`). These are small fixture measurements, so enforce page,
request, and wall-clock ceilings; return partial fetched data with
coverage when a ceiling is reached. Parse `-i` status and pagination
headers, and treat GraphQL `errors` and page cursors explicitly.

Direct HTTP was about twice as fast in repeated project reads and both
two-page runs. It remains a contingency for a measured stats backfill
that exceeds the 60-second budget. A direct adapter needs a durable way
to obtain the host's current credential, refresh OAuth, and preserve
keychain support. `glab config get oauth_token` sufficed for this probe,
but a token snapshot can expire. GitLab documents environment-token
precedence, keyring or plaintext storage, and OAuth refresh failures
when credential storage is unwritable in its [authentication guide](https://docs.gitlab.com/cli/authentication/).
No credential belongs in logs, SQLite, process arguments, or phone traffic.

This decision is **GitLab.com first**. The user has no self-managed test
instance yet. Host version, custom API path, OAuth application setup,
permissions, and tier capabilities remain unvalidated; do not infer
support on self-managed hosts from these GitLab.com observations.

## Reproduce a bounded, redacted read probe

The following procedure requests only the disposable project's two MR
pages. It captures bodies in memory, prints no raw content or credential,
and makes no remote changes. Use `glab`'s existing authentication; do
not set a token in the shell command. Add further endpoints only with
an explicit host and project path and a bounded page range.

```bash
python3 - <<'PY'
import json
import subprocess
import time

root = 'projects/<url-encoded-project-path>/merge_requests'
for page in (1, 2):
    path = f'{root}?state=opened&per_page=1&page={page}'
    start = time.monotonic()
    result = subprocess.run(
        ['glab', 'api', '--hostname', 'gitlab.com', path],
        capture_output=True, text=True, timeout=15, check=False,
    )
    try:
        body = json.loads(result.stdout)
    except json.JSONDecodeError:
        body = None
    print({'page': page, 'exit': result.returncode,
           'elapsed_ms': round((time.monotonic() - start) * 1000),
           'kind': type(body).__name__,
           'items': len(body) if isinstance(body, list) else None,
           'error_body_present': bool(result.stderr)})
PY
```

## Remaining fixture matrix

| Surface | GitLab.com fixture | Self-managed fixture |
| --- | --- | --- |
| Project lookup, two populated MR pages, missing-IID 404 | Observed | Deferred |
| Authored queue with two drafts | Observed | Deferred |
| Reviewer queue with assigned reviewer | Needed; current queue empty | Deferred |
| MR detail and approvals with no required rules | Observed | Deferred |
| Required approval rule, actual approval, tier/permission denial | Needed | Deferred |
| Resolved diff discussion plus system notes | Observed | Deferred |
| One successful MR pipeline and job | Observed | Deferred |
| Merge-train capability | Needed | Deferred |
| Larger stats backfill and partial timeout | Needed | Deferred |
| Expired credential, rate limit, 5xx | Needed | Deferred |

For each future fixture, preserve a redacted schema (field names, JSON
types, nullability, status, relevant header names), latency, and
coverage. A zero-item page is a valid observation; it is not evidence
that the feature is unsupported or that a queue's total is zero across
all scopes.
