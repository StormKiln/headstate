# GitLab slice 7: permission checks and verified actions

Author: Astra / high. GitLab.com remains the supported host; self-managed
hosts are refused before invoking the CLI. Every command carries the full
provider, host, project path and MR IID, including desktop and phone requests.
GitLab actions have their own command surface and cannot fall through to a
GitHub mutation. Credentials remain owned by `glab` on the desktop.

## Permission and write contract

Before each write, the adapter reads the exact MR and its GraphQL permissions.
The REST IID/URL and GraphQL IID/URL/head must agree with the requested identity.
Approval additionally requires the REST `user_can_approve` flag and refuses
known reauthentication requirements. Discussion resolution checks each
discussion's `resolveNote` permission; replies require an explicitly non-system
first note with `createNote` permission as well as MR comment permission. Missing
note or permission data suppresses replies. Retry CI checks the current-head
pipeline's `retryable` and `updatePipeline` flags. Pipeline writes use the
pipeline's project ID, including fork pipelines.

The UI offers approve, comment, discussion reply/resolve/reopen, merge,
close/reopen, draft/ready, source-branch rebase, retry CI and supported
auto-merge only when the fetched permission receipt allows them. Merge
requires the raw detailed status `mergeable`. Merge trains and GitHub
request-changes reviews are explained as unsupported. Auto-merge requires
GitLab to advertise `merge_when_checks_pass`; a merge train strategy alone
does not enable it. Approval, merge and auto-merge send the displayed head SHA
to GitLab. Rebase and CI retry recheck that head before issuing their write;
GitLab's rebase endpoint has no atomic head guard, which the confirmation names.

Every supported action has a separate readback. Approval checks the current
user's approval and head; comments/replies check the returned note ID and body;
resolution checks the requested thread's resolvable notes; state changes read
the MR again; auto-merge reads its enabled flag. CI retry captures the newest 100 current jobs before writing and rechecks the
MR head and pipeline identity immediately before the POST. It polls jobs at
most three times afterward. A new job ID above the baseline maximum must match
the name and stage of a captured failed or canceled job, with the same pipeline,
project and SHA. The receipt confirms only **at least one** new attempt;
protected jobs may still have been skipped. An unchanged active pipeline,
unrelated new job, missing baseline, failed readback or expired budget remains
unverified. Rebase polls at most three times and
requires a changed head matching GitLab's `rebaseCommitSha` after completion.

A successful HTTP status alone never yields `verified`. A write timeout,
refusal, failed readback or operation still pending yields `unverified`, with
instructions to refresh before acting again. The app never automatically
retries a write. Text goes through CLI stdin, not process arguments; comments
beginning with slash commands are refused so GitLab quick actions cannot
bypass the dedicated action gates.

## Bounds and bulk behavior

Each action has a 60-second budget; each subprocess is limited to 15 seconds
and 4 MiB of output and is killed on cancellation. The permissions read asks
for at most 100 discussions. Later discussions are explicitly left for
GitLab's UI. Comment input is capped at 32 KiB.

Bulk selection uses complete identities and is separate from GitHub selection.
It accepts at most ten GitLab rows and offers approve, close, draft and ready
only when all selected rows have matching permission/head receipts. The
confirmation enumerates host, project and IID. Each row is rechecked at write
time, requests run sequentially, and results are retained separately. All
attempted rows are deselected afterwards because an uncertain write may have
landed. Merge, auto-merge and rebase are not bulk actions.

## Evidence and limitations

A bounded read of the disposable GitLab.com fixture verified permission field
names, pipeline permission shape, the `merge_when_checks_pass` strategy and
GraphQL discussion IDs (`DiffDiscussion` / `IndividualNoteDiscussion` followed
by a 40-character hash). The adapter strips only recognized type prefixes to
obtain the REST discussion ID. Public schema introspection verified
`MergeRequestSetDraftInput`; draft toggles use that mutation rather than
rewriting an MR title. No live write, merge or fixture deletion was performed.

Synthetic tests cover 13 actions with successful and failed readback, plus CI
retry with new failed/canceled job attempts, delayed readback, unchanged active
pipelines, unrelated jobs, malformed and missing evidence, bounded-page
high-water marks, identity mismatches, budget exhaustion, permission denial/missing data, stale heads, unsupported
hosts, identity mismatches, GraphQL errors, quick-action refusal, bounded
discussion permissions and provider-qualified UI/bulk dispatch. CI retry uses GitLab's documented newest-first job ordering and new retry IDs;
jobs outside the captured page, trigger-only retries and evidence that arrives
after the bounded read remain unverified. Concurrent retries by another actor
can supply the observed new attempt; the receipt proves resulting state, not
which actor caused it. A no-op rebase can leave the SHA unchanged and remains
unverified. A bounded
read of the same disposable fixture confirmed system notes can expose
`createNote: true`, so system status is checked independently. Backend fixtures
prove system/unknown discussion permission is refused before the POST; UI
fixtures prove the reply form is absent while ordinary replies remain usable. Required approval rules,
reauthentication, merge trains and self-managed hosts still lack live fixtures.

Contracts checked against GitLab's [merge request API](https://docs.gitlab.com/api/merge_requests/),
[approval API](https://docs.gitlab.com/api/merge_request_approvals/),
[discussion API](https://docs.gitlab.com/api/discussions/),
[notes API](https://docs.gitlab.com/api/notes/),
[jobs API](https://docs.gitlab.com/api/jobs/),
[job retry behavior](https://docs.gitlab.com/ci/jobs/#retry-jobs),
[pipeline retry API](https://docs.gitlab.com/api/pipelines/#retry-jobs-in-a-pipeline) and
[GraphQL schema](https://docs.gitlab.com/api/graphql/reference/).
