# GitLab integration: merge-readiness record

This records the follow-up to PR #1596, starting at `7a7a9d5`.
Automated checks and the scoped GitLab.com follow-up below are separate evidence; neither establishes a self-managed version range.

## Behavior contract

| Workflow | GitHub / Both | GitLab / Both | Paired mobile |
| --- | --- | --- | --- |
| Queues and refresh | Existing GitHub hooks, receipts and retry suppression | Independent authored/reviewing receipts; core rows publish before optional evidence | Desktop owns provider reads; source receipts mirrored |
| Search, filters, sorting, density | Existing filter implementation and row | Same controls over measured GitLab evidence; unknown never matches success | Shared frontend |
| Selection and bulk actions | Existing BulkBar, range selection, keyboard selection retained in Both | Up to ten MRs, fresh per-MR permissions and head checks, individual outcomes | Same commands and remote authorization policy |
| Overview / review requests | Existing priority, readiness and review strips and request-review dialog retained in Both | CI, approval/reviewer, discussions and merge-train evidence | GitHub desktop-only features keep their original mobile gates |
| Detail | Existing PrDetailView unchanged | Markdown, discussions, approvals/rules, pipeline/jobs, qualified partial reads | Shared detail and action UI; touch targets |
| Writes | Existing GitHub pipeline | Approve, request changes, comment/reply, resolve/unresolve, merge, close/reopen, draft/ready, rebase, retry CI, auto-merge and merge train; unavailable capabilities stay disabled | Existing read/write/step-up classification retained |
| Uncertain writes | Existing behavior | No automatic retry; explicit acknowledgment required before another action; both queues reconciled | Invalidation event reaches connected clients |
| Branch relationships | Existing GitHub stack markers | Parent branch marker scoped by host and complete project path; no invented merge-readiness verdict | Shared list |
| Local repositories / Claudify | Existing checkout spelling retained for github.com | Other hosts retain host and full namespace; reuse validated checkout, command quoting, preview and launch flow | Copy/show command flow; launching a terminal remains desktop-only |
| Statistics | Existing Github statistics stay separate | Scope navigation, author lookup, daily chart/table, author and reviewer measures, resumable history | Shared statistics UI where the existing mobile view capability permits it |
| Notifications | Existing scheduler unchanged | Same-head known CI transitions; initial/changed-account observations establish a silent baseline | Desktop notification owner |

Platform differences are explicit: merge trains and approval rules depend on
server version/tier/configuration. GitLab's request-changes and rebase APIs do
not accept an atomic expected-head guard; these use fresh head checks and
independent readback. Current approval snapshots and reviewer assignments are
not a complete history of formal reviews and are not labelled first-review
latency. An unavailable optional schema field does not justify disabling
unrelated supported reads or basic actions.

## Cache and request policy

- GitHub keeps its existing cache keys and fetch coordination. GitLab never
  writes GitHub receipts, detail keys or statistics totals.
- Fresh queue snapshots seed mounts without an unconditional refresh. Desktop
  polling keeps the configured cadence; mobile foreground refreshes use the
  same desktop coordinator. Identical in-flight queue loads share a sequence.
- Rows carry the verified GitLab username. Frontend queue/detail/action caches
  are account-scoped; late or saved rows from another account are rejected.
  Unknown authentication does not expose a prior account's rows. Auth is
  rechecked once per minute while GitLab is selected and on query revalidation;
  external credential changes cannot be detected while disconnected.
- Detail freshness is 60 seconds and capability freshness 30 seconds in the
  webview. Simultaneous desktop/phone detail and capability reads coalesce.
  Execution always obtains fresh permissions/head evidence, regardless of UI
  freshness. Identity probes remain separate bounded reads owned by `glab`.
- Queue evidence uses a 60-second cache keyed by full MR identity, account,
  head, update time and per-MR invalidation revision. Refreshing after a write
  invalidates that MR's evidence, retaining other MRs' fresh evidence.
- Statistics use the existing five-minute desktop cache and five-minute
  frontend freshness, with 30-minute frontend retention. Normal navigation
  does not force a reload. Explicit Refresh bypasses the report cache once.
  Scope-tree freshness survives remounts. Statistics keys include host,
  numeric account ID, scope and UTC window. Account identity is rechecked
  before publishing/persisting newly fetched reports and history.
- Mutations invalidate affected detail/capability queries and host report
  caches, without refetching the unchanged scope tree. Both queue memberships
  refresh after verified, rejected or uncertain outcomes. A publication
  barrier rejects pre-mutation receipts even if reconciliation fails.
- Queue enrichment considers at most 20 missing/expired MRs per refresh, four
  at a time; oldest/unvisited rows go first. Known unsupported optional schema
  fields are omitted for ten minutes. A 429 stops further enrichment for that
  host for at least 60 seconds; already-running requests may finish.
- Provider subprocess stdout is capped at 4 MiB while streaming; stderr is
  discarded. Queues retain at most five 100-row pages. Statistics retain at
  most ten pages and 16 MiB of raw page bytes. Detail pagination retains its
  existing five-page bounds. These are wire-byte limits, not a promise that
  parsed JSON consumes the same amount of heap.
- A four-request semaphore bounds GitLab CLI resource calls. Queue pages and
  enrichment share the existing 30-second resource budget; identity checks
  have separate 15-second bounds. The test fix did not raise production
  deadlines or serialize/skip tests.

## Diagnosis and fixes

The original CI pagination assertion depended on three subprocess launches
finishing within two seconds. The same family failed in normally parallel
local tests while passing serially. Scripted subprocesses now signal the
intended timeout boundary before a controlled Tokio clock advances; ordinary
OS scheduling cannot silently consume a pagination fixture's budget.

Three production regressions were reproduced and fixed: detail writes left
queue membership stale, modern retry statuses exposed a transient GitHub
error too early, and queue/statistics commands buffered unbounded output.

The review additionally found and fixed account-crossing caches, hidden
repository filter intersections, missing GitLab auth diagnostics, uncertain
comment resubmission, stale pre-write publications, optional-schema coupling,
and host/subgroup loss in local checkout identities.

The mobile Stronghold persistence test was spending time in unoptimized
scrypt/Salsa20. Test-profile optimization of those two dependencies makes the
unchanged persistence/wrong-key test practical; production work factors are
unchanged.

## Verification and remaining gate

Local checks passed during final integration: 4,581 UI tests (309 files),
2,974 desktop Rust tests (50 pre-existing ignored), 20 step-up tests and
224 mobile tests. Complete lint, mobile lint, desktop and mobile frontend
builds passed. An independent review found no remaining actionable issue
after its regression fixes. The oversized-response tests passed together in
0.60 seconds with a measured 61,538,304-byte maximum RSS on this macOS host;
that is a fixture-process measurement, not a whole-application memory bound.
Final commit and cross-platform CI evidence are recorded in the PR update.
Required checks: `make test-ui`, normally parallel `make test-rust` (repeated),
`make lint`, `make test-mobile`, `make lint-mobile`, desktop and mobile frontend
builds, and final-head CI across supported platforms.

### Live GitLab.com follow-up

Tested with GitLab.com reporting `19.5.0-pre` (revision `856fb0ad1c5`) and
`glab 1.120.0` on macOS. Two private fictional projects, six MRs, labels,
discussions and CI pipelines are retained for future desktop/phone screenshots.
The authenticated owner explicitly requested that this data not be deleted.

The probe invoked HeadState's actual Rust queue, detail, capability, statistics
and action functions. It found and fixed two issues fixtures had missed:

- The change-request connection is `changeRequesters`, not `changeRequestedBy`.
- `glab` exits nonzero for GraphQL error documents returned with HTTP status
  200. Preserve these typed errors for schema fallback on GraphQL calls only;
  HTTP and REST failures remain failures. A subprocess regression failed before
  the parser fix and passed afterward.

Both queue types loaded measured CI, reviewer and discussion evidence. Detail
reads and permission checks succeeded. Comment, reply, resolve/unresolve,
draft/ready, close/reopen, merge and CI retry all returned independently verified
receipts. A real rebase completed after the bounded immediate readback, so the
application correctly returned an unverified receipt; a later read confirmed
that the new head matched `rebaseCommitSha`, with no rebase running or error.
No uncertain write was automatically retried.

Measured calls in one live probe: authored queue 9, reviewing queue 4 (the
second list reused all five open MRs' fresh enrichment); explicit statistics
refresh 16, subsequent cached load 1 (identity check only, no report refetch).
These are observations for this small dataset, not large-project benchmarks.

### Remaining live gates

- Approval and request-changes writes need a second test identity to create or
  review another author's MR. The owner cannot exercise those on its own MRs.
- Merge trains and other unavailable tier/configuration features need an
  appropriately configured test project. Unavailable actions remain disabled.
- Self-managed compatibility needs an authorized instance and tested version
  floor. GitLab.com's development version is not a self-managed support claim.
- Paired-device authorization, disconnect/resume and native desktop/phone UI
  smoke tests remain open. Browser layout tests do not establish device parity.
- Live account switching, cross-provider failure isolation, nested namespaces
  and larger-data performance remain outside this single-account smoke run.

Retain all synthetic repositories, branches, MRs, comments and pipeline history.
Record additional live results and resolve failures before treating these gates
as complete.
